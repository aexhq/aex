//! Statement download grants and the statement list cursor.
//!
//! A grant and the signature inside it expire together (OD-17): the presigned
//! URL is minted for exactly the configured lifetime, and the configuration
//! refuses a lifetime longer than five minutes. A leaked signature therefore
//! cannot outlive the grant that authorised it.

use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::types::HttpsUrl;
use base64::Engine as _;

/// What a presigner answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignedObject {
    /// The signed URL. Never logged and never journaled.
    pub url: HttpsUrl,
    /// When the signature stops verifying, in epoch milliseconds.
    pub expires_at_millis: i64,
    /// The object's exact byte length.
    pub size_bytes: u128,
}

/// Why a statement artifact could not be signed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DownloadError {
    /// The artifact does not exist in the bucket.
    #[error("the statement artifact is absent")]
    Absent,
    /// The artifact store is unreachable.
    #[error("the statement artifact store is unavailable: {0}")]
    Unavailable(String),
    /// The signature could not be produced.
    #[error("the statement artifact could not be signed: {0}")]
    Unsignable(String),
}

/// Mints a bounded read grant over one immutable statement artifact.
#[async_trait::async_trait]
pub trait StatementDownloads: Send + Sync + 'static {
    /// Signs a read of `object_key` for `ttl_millis`.
    async fn presign(
        &self,
        object_key: &str,
        ttl_millis: i64,
    ) -> Result<PresignedObject, DownloadError>;
}

/// The S3 implementation.
#[derive(Debug, Clone)]
pub struct S3StatementDownloads {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3StatementDownloads {
    /// Builds the presigner for one bucket.
    #[must_use]
    pub const fn new(client: aws_sdk_s3::Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    /// The bucket this presigner is bound to.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }
}

#[async_trait::async_trait]
impl StatementDownloads for S3StatementDownloads {
    async fn presign(
        &self,
        object_key: &str,
        ttl_millis: i64,
    ) -> Result<PresignedObject, DownloadError> {
        let ttl = std::time::Duration::from_millis(u64::try_from(ttl_millis).unwrap_or(0));
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|error| {
                DownloadError::Unavailable(
                    aws_sdk_s3::error::DisplayErrorContext(&error).to_string(),
                )
            })?;
        let size_bytes = u128::try_from(head.content_length().unwrap_or_default())
            .map_err(|_| DownloadError::Absent)?;
        let config = aws_sdk_s3::presigning::PresigningConfig::expires_in(ttl)
            .map_err(|error| DownloadError::Unsignable(error.to_string()))?;
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(object_key)
            .presigned(config)
            .await
            .map_err(|error| {
                DownloadError::Unsignable(
                    aws_sdk_s3::error::DisplayErrorContext(&error).to_string(),
                )
            })?;
        let url = HttpsUrl::parse(request.uri())
            .map_err(|error| DownloadError::Unsignable(error.to_string()))?;
        let expires_at_millis = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(0)
            + ttl_millis;
        Ok(PresignedObject {
            url,
            expires_at_millis,
            size_bytes,
        })
    }
}

/// Encodes a statement period as an opaque continuation token.
///
/// The statement list is keyset-paged on `period DESC`, and the period is the
/// unique sort key inside one organization, so the position is the whole state
/// the cursor needs to carry.
///
/// # Errors
///
/// Returns `internal_error` when the encoded token is not a valid cursor, which
/// cannot happen for a `YYYY-MM` period.
pub fn encode_period_cursor(period: &str) -> WireResult<Cursor> {
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(period.as_bytes());
    Cursor::parse(&format!("{}{body}", Cursor::PREFIX)).map_err(|error| {
        WireError::new(ErrorCode::InternalError)
            .with_message(format!("the statement cursor is unrepresentable: {error}"))
    })
}

/// Decodes a statement continuation token back into a period.
///
/// # Errors
///
/// Returns `invalid_cursor` for anything that is not a token this deployable
/// minted: a wrong prefix, a body that is not base64url, bytes that are not
/// UTF-8, or a value that is not a `YYYY-MM` period.
pub fn decode_period_cursor(cursor: &Cursor) -> WireResult<String> {
    let invalid = || WireError::new(ErrorCode::InvalidCursor);
    let body = cursor
        .as_str()
        .strip_prefix(Cursor::PREFIX)
        .ok_or_else(invalid)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| invalid())?;
    let period = String::from_utf8(raw).map_err(|_| invalid())?;
    if !is_period(&period) {
        return Err(invalid());
    }
    Ok(period)
}

/// Whether `value` is a `YYYY-MM` calendar month.
fn is_period(value: &str) -> bool {
    let Some((year, month)) = value.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month.len() == 2
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
}

#[cfg(test)]
mod tests {
    use super::{decode_period_cursor, encode_period_cursor, is_period};
    use aex_wire::cursor::Cursor;
    use aex_wire::error::ErrorCode;

    #[test]
    fn a_period_cursor_round_trips() {
        let cursor = encode_period_cursor("2026-07").expect("a valid period encodes");
        assert!(cursor.as_str().starts_with(Cursor::PREFIX));
        assert_eq!(
            decode_period_cursor(&cursor).expect("the token decodes"),
            "2026-07"
        );
    }

    #[test]
    fn a_token_this_deployable_did_not_mint_is_an_invalid_cursor() {
        let forged = Cursor::parse("cur_notbase64___").expect("the envelope grammar accepts it");
        let error = decode_period_cursor(&forged).expect_err("the body is not a period");
        assert_eq!(error.code, ErrorCode::InvalidCursor);
    }

    #[test]
    fn a_period_is_exactly_four_digits_a_dash_and_a_month() {
        assert!(is_period("2026-01"));
        assert!(is_period("2026-12"));
        assert!(!is_period("2026-13"));
        assert!(!is_period("2026-1"));
        assert!(!is_period("26-01"));
        assert!(!is_period("2026"));
    }
}
