//! Partial-batch handling and poison quarantine.
//!
//! A batch reports **exactly** the identifiers that failed. Reporting the whole
//! batch would redrive the ones that already succeeded, which for a lifecycle
//! effect means dispatching it twice.

use serde::{Deserialize, Serialize};

/// How many delivery attempts a message gets before it is quarantined.
pub const MAX_RECEIVE_COUNT: u32 = 5;

/// One item's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemOutcome {
    /// Handled. Delete it.
    Succeeded,
    /// Failed in a way that a redrive can fix.
    Retryable {
        /// Why.
        reason: String,
    },
    /// Failed in a way a redrive cannot fix. Quarantine with an operator record
    /// and an alarm; never redrive hot.
    Poison {
        /// Why.
        reason: String,
    },
}

/// One message in a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    /// The message identity.
    pub message_id: String,
    /// How many times it has been delivered.
    pub receive_count: u32,
    /// What happened.
    pub outcome: ItemOutcome,
}

/// One identifier in a partial-batch response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemFailure {
    /// The message identity.
    pub item_identifier: String,
}

/// The partial-batch response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialBatchFailure {
    /// Exactly the identifiers that must be redriven.
    pub batch_item_failures: Vec<BatchItemFailure>,
}

/// An item that was quarantined rather than redriven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantined {
    /// Which message.
    pub message_id: String,
    /// Why.
    pub reason: String,
    /// How many deliveries it had.
    pub receive_count: u32,
}

/// What one batch produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchResult {
    /// The partial-batch response to return.
    pub response: PartialBatchFailure,
    /// The items that were quarantined, each of which needs an operator record and
    /// an alarm.
    pub quarantined: Vec<Quarantined>,
}

/// Folds one batch into a response.
#[must_use]
pub fn fold_batch(items: &[BatchItem]) -> BatchResult {
    let mut response = PartialBatchFailure::default();
    let mut quarantined = Vec::new();
    for item in items {
        match &item.outcome {
            ItemOutcome::Succeeded => {}
            ItemOutcome::Poison { reason } => quarantined.push(Quarantined {
                message_id: item.message_id.clone(),
                reason: reason.clone(),
                receive_count: item.receive_count,
            }),
            ItemOutcome::Retryable { reason } => {
                if item.receive_count >= MAX_RECEIVE_COUNT {
                    quarantined.push(Quarantined {
                        message_id: item.message_id.clone(),
                        reason: format!("{reason} (exhausted {MAX_RECEIVE_COUNT} deliveries)"),
                        receive_count: item.receive_count,
                    });
                } else {
                    response.batch_item_failures.push(BatchItemFailure {
                        item_identifier: item.message_id.clone(),
                    });
                }
            }
        }
    }
    BatchResult {
        response,
        quarantined,
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchItem, ItemOutcome, MAX_RECEIVE_COUNT, PartialBatchFailure, fold_batch};

    fn item(id: &str, receive_count: u32, outcome: ItemOutcome) -> BatchItem {
        BatchItem {
            message_id: id.to_owned(),
            receive_count,
            outcome,
        }
    }

    #[test]
    fn only_the_failed_identifiers_are_reported() {
        let batch = [
            item("a", 1, ItemOutcome::Succeeded),
            item(
                "b",
                1,
                ItemOutcome::Retryable {
                    reason: "throttled".to_owned(),
                },
            ),
            item("c", 1, ItemOutcome::Succeeded),
        ];
        let result = fold_batch(&batch);
        assert_eq!(
            result
                .response
                .batch_item_failures
                .iter()
                .map(|failure| failure.item_identifier.as_str())
                .collect::<Vec<_>>(),
            vec!["b"],
            "redriving a succeeded lifecycle effect would dispatch it twice"
        );
        assert!(result.quarantined.is_empty());
    }

    #[test]
    fn a_poison_item_quarantines_and_never_redrives_hot() {
        let batch = [item(
            "a",
            1,
            ItemOutcome::Poison {
                reason: "malformed payload".to_owned(),
            },
        )];
        let result = fold_batch(&batch);
        assert!(
            result.response.batch_item_failures.is_empty(),
            "a poison item must not be redriven"
        );
        assert_eq!(result.quarantined.len(), 1);
        assert_eq!(result.quarantined[0].message_id, "a");
    }

    #[test]
    fn an_exhausted_retry_budget_quarantines_rather_than_looping() {
        let batch = [item(
            "a",
            MAX_RECEIVE_COUNT,
            ItemOutcome::Retryable {
                reason: "provider transient".to_owned(),
            },
        )];
        let result = fold_batch(&batch);
        assert!(result.response.batch_item_failures.is_empty());
        assert_eq!(result.quarantined.len(), 1);
        assert!(result.quarantined[0].reason.contains("exhausted"));
    }

    #[test]
    fn an_all_succeeded_batch_reports_nothing() {
        let batch = [
            item("a", 1, ItemOutcome::Succeeded),
            item("b", 1, ItemOutcome::Succeeded),
        ];
        assert_eq!(fold_batch(&batch).response, PartialBatchFailure::default());
    }

    #[test]
    fn the_response_serializes_with_the_shape_the_service_expects() {
        let batch = [item(
            "a",
            1,
            ItemOutcome::Retryable {
                reason: "x".to_owned(),
            },
        )];
        let json = serde_json::to_value(fold_batch(&batch).response).expect("it serializes");
        assert_eq!(
            json,
            serde_json::json!({ "batchItemFailures": [{ "itemIdentifier": "a" }] })
        );
    }
}
