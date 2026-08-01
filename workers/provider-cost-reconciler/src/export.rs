//! Reading one provider cost export from object storage.
//!
//! The parsers are driven only by committed sanitized fixtures: the real export
//! formats are private commercial data, so a row this module cannot parse is a
//! hard refusal rather than a guess.

use aex_wire::types::DecimalU128;

use crate::cost::{CostError, ProviderCostRow, ProviderCostSource};
use crate::handler::CostExport;

/// The S3-backed export reader.
#[derive(Debug, Clone)]
pub struct S3CostExport {
    client: aws_sdk_s3::Client,
    bucket: String,
    prefix: String,
}

impl S3CostExport {
    /// Builds the reader for one bucket and prefix.
    #[must_use]
    pub const fn new(client: aws_sdk_s3::Client, bucket: String, prefix: String) -> Self {
        Self {
            client,
            bucket,
            prefix,
        }
    }

    /// The object key one period's export is delivered under.
    #[must_use]
    pub fn object_key(&self, period: &str) -> String {
        format!("{}{period}/cost.jsonl", self.prefix)
    }
}

/// Parses one JSON-lines export body into normalised rows.
///
/// # Errors
///
/// Returns [`CostError::OffContract`] for a line this build cannot parse, a
/// negative cost, or a cost that is not an integer micro-USD amount. There is
/// deliberately no arm that skips a line: a silently dropped cost row makes the
/// margin look better than it is.
pub fn parse(source: ProviderCostSource, body: &str) -> Result<Vec<ProviderCostRow>, CostError> {
    let mut rows = Vec::new();
    for (index, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: ProviderCostRow = serde_json::from_str(line).map_err(|error| {
            CostError::OffContract(format!("line {} does not parse: {error}", index + 1))
        })?;
        if row.source != source {
            return Err(CostError::OffContract(format!(
                "line {} declares source `{}`, not `{}`",
                index + 1,
                row.source.as_str(),
                source.as_str()
            )));
        }
        if row.cost_microusd < 0 {
            return Err(CostError::OffContract(format!(
                "line {} reports a negative cost",
                index + 1
            )));
        }
        rows.push(row);
    }
    Ok(rows)
}

#[async_trait::async_trait]
impl CostExport for S3CostExport {
    async fn rows(
        &self,
        period: &str,
        max_scan_bytes: u64,
    ) -> Result<Vec<ProviderCostRow>, CostError> {
        let key = self.object_key(period);
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| {
                CostError::ExportUnavailable(
                    aws_sdk_s3::error::DisplayErrorContext(&error).to_string(),
                )
            })?;
        let size = u64::try_from(head.content_length().unwrap_or_default()).unwrap_or(u64::MAX);
        if size > max_scan_bytes {
            // A scan budget that a run silently exceeds is not a budget.
            return Err(CostError::OffContract(format!(
                "the export for `{period}` is {} bytes, above the {max_scan_bytes}-byte scan budget",
                DecimalU128::new(u128::from(size))
            )));
        }
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| {
                CostError::ExportUnavailable(
                    aws_sdk_s3::error::DisplayErrorContext(&error).to_string(),
                )
            })?;
        let bytes = object.body.collect().await.map_err(|error| {
            CostError::ExportUnavailable(format!("the export body could not be read: {error}"))
        })?;
        let body = String::from_utf8(bytes.to_vec())
            .map_err(|_| CostError::OffContract("the export is not UTF-8".to_owned()))?;
        parse(ProviderCostSource::AwsCur, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderCostSource, parse};

    const SANITIZED: &str = r#"
{"source":"aws_cur","sourceRowId":"row-1","period":"2026-07","service":"AmazonECS","region":"eu-west-1","costMicrousd":1250000}
{"source":"aws_cur","sourceRowId":"row-2","period":"2026-07","service":"AmazonS3","region":"eu-west-1","costMicrousd":42000}
"#;

    #[test]
    fn a_sanitized_export_parses_into_integer_micro_usd_rows() {
        let rows = parse(ProviderCostSource::AwsCur, SANITIZED).expect("the fixture parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cost_microusd, 1_250_000);
        assert_eq!(rows[1].source_row_id, "row-2");
    }

    #[test]
    fn a_fractional_cost_is_refused_rather_than_rounded() {
        let raw = r#"{"source":"aws_cur","sourceRowId":"r","period":"2026-07","service":"s","region":"eu-west-1","costMicrousd":1.5}"#;
        assert!(
            parse(ProviderCostSource::AwsCur, raw).is_err(),
            "money never crosses this boundary as a float"
        );
    }

    #[test]
    fn a_negative_cost_is_refused() {
        let raw = r#"{"source":"aws_cur","sourceRowId":"r","period":"2026-07","service":"s","region":"eu-west-1","costMicrousd":-1}"#;
        assert!(parse(ProviderCostSource::AwsCur, raw).is_err());
    }

    #[test]
    fn a_row_from_another_export_family_is_refused() {
        let raw = r#"{"source":"stripe_balance_report","sourceRowId":"r","period":"2026-07","service":"s","region":"eu-west-1","costMicrousd":1}"#;
        assert!(parse(ProviderCostSource::AwsCur, raw).is_err());
    }

    #[test]
    fn an_unparseable_line_fails_the_run_rather_than_being_skipped() {
        let raw = "{\"source\":\"aws_cur\"}\n";
        assert!(
            parse(ProviderCostSource::AwsCur, raw).is_err(),
            "a silently dropped cost row makes the margin look better than it is"
        );
    }
}
