//! The `ProjectionStore` implementation over `usage-query-projection`.
//!
//! The projection is the one table in this stream that is *not* charge truth: it
//! is derived, generation-keyed and rebuildable. That is why this adapter may
//! update rows in place while the authority adapter may not.
//!
//! The fold itself is pure and lives in [`aex_usage_application::projection`].
//! This module only renders a [`ProjectionTransaction`] into one
//! `TransactWriteItems`, verbatim: every `ADD`, every `SET` and every condition
//! comes from the fold, so a coverage fence cannot be dropped on the way to the
//! table.
//!
//! # Why a refused condition is not an error
//!
//! `TransactWriteItems` is all-or-nothing. A redelivered stream record fails the
//! coverage condition, writes nothing, and is reported as
//! [`Applied::AlreadyCovered`] — an expected condition, not a fault. Reporting
//! it as a failure would make a normal Lambda redelivery look like an outage.

use aex_usage_application::ports::{Applied, CoverageAdvance, PortError, ProjectionStore};
use aex_usage_application::projection::{ProjectionTransaction, ProjectionWrite, Sign};
use aex_usage_domain::projection::{Generation, ProjectionKeys};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{AttributeValue, TransactWriteItem, Update};

use crate::attribute;
use crate::fault::{Idempotence, classify};

/// The name this store reports in a port error.
const WHAT: &str = "usage projection";

/// The attribute the generation pointer row carries.
pub const GENERATION_ATTRIBUTE: &str = "generation";

/// The shared, rebuildable query projection.
#[derive(Debug, Clone)]
pub struct QueryProjection {
    client: Client,
    table: String,
    keys: ProjectionKeys,
}

impl QueryProjection {
    /// Binds the projection to a client and a physical table name.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
            keys: ProjectionKeys,
        }
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Renders one folded write as a conditional update.
    fn update(&self, write: &ProjectionWrite) -> Result<TransactWriteItem, PortError> {
        let mut names: Vec<(String, String)> = Vec::new();
        let mut values: Vec<(String, AttributeValue)> = Vec::new();
        let mut set_clauses: Vec<String> = Vec::new();
        let mut add_clauses: Vec<String> = Vec::new();

        // The discriminator is written by the adapter rather than by the fold:
        // the fold names a row kind, and what that kind is called on the table
        // is this adapter's business.
        names.push(("#itemType".to_owned(), attribute::ITEM_TYPE.to_owned()));
        values.push((
            ":itemType".to_owned(),
            attribute::text(write.row.item_type()),
        ));
        set_clauses.push("#itemType = :itemType".to_owned());

        for (index, (name, value)) in write.set.iter().enumerate() {
            let placeholder = format!("#s{index}");
            let binding = format!(":s{index}");
            names.push((placeholder.clone(), name.clone()));
            values.push((binding.clone(), attribute::attribute(value)));
            set_clauses.push(format!("{placeholder} = {binding}"));
        }
        for (index, (name, delta)) in write.add.iter().enumerate() {
            let placeholder = format!("#a{index}");
            let binding = format!(":a{index}");
            names.push((placeholder.clone(), name.clone()));
            // `ADD` takes a signed literal, so a withdrawal is one clause rather
            // than a read-modify-write that could lose a concurrent addition.
            values.push((
                binding.clone(),
                AttributeValue::N(match delta.sign {
                    Sign::Add => delta.magnitude.to_string(),
                    Sign::Subtract => format!("-{}", delta.magnitude),
                }),
            ));
            add_clauses.push(format!("{placeholder} {binding}"));
        }

        let mut expression = format!("SET {}", set_clauses.join(", "));
        if !add_clauses.is_empty() {
            use std::fmt::Write as _;
            write!(expression, " ADD {}", add_clauses.join(", ")).map_err(|error| {
                PortError::Corrupt {
                    what: "projection write",
                    reason: error.to_string(),
                }
            })?;
        }

        // The fold owns the condition and the values it binds; the adapter
        // renders them verbatim so a coverage fence cannot be dropped here.
        for (binding, value) in write.condition.values() {
            values.push((binding, attribute::attribute(&value)));
        }

        let key = aws_sdk_dynamodb::types::AttributeValue::S(write.key.pk.clone());
        let sort = write.key.sk.clone().ok_or_else(|| PortError::Corrupt {
            what: "projection key",
            reason: "a folded write always names a sort key".to_owned(),
        })?;

        let update = Update::builder()
            .table_name(&self.table)
            .key(attribute::PARTITION, key)
            .key(attribute::SORT, AttributeValue::S(sort))
            .update_expression(expression)
            .condition_expression(write.condition.expression())
            .set_expression_attribute_names(Some(names.into_iter().collect()))
            .set_expression_attribute_values(Some(values.into_iter().collect()))
            .build()
            .map_err(|error| PortError::Corrupt {
                what: "projection write",
                reason: error.to_string(),
            })?;
        Ok(TransactWriteItem::builder().update(update).build())
    }
}

#[async_trait]
impl ProjectionStore for QueryProjection {
    async fn current_generation(&self) -> Result<Generation, PortError> {
        let key = self.keys.generation_pointer();
        let sort = key.sk.clone().unwrap_or_default();
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(attribute::PARTITION, attribute::text(key.pk))
            .key(attribute::SORT, attribute::text(sort))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Read, &error))?;

        // An absent pointer is not a failure to fall back from: a table that has
        // never been rebuilt is serving its first generation, and that is what
        // `G0000` means.
        let Some(item) = output.item else {
            return Ok(Generation::FIRST);
        };
        let raw = item
            .get(GENERATION_ATTRIBUTE)
            .ok_or(PortError::Corrupt {
                what: "generation pointer",
                reason: "the pointer row carries no generation".to_owned(),
            })?
            .as_n()
            .map_err(|_| PortError::Corrupt {
                what: "generation pointer",
                reason: "the generation is not a number".to_owned(),
            })?;
        let value = raw.parse::<u16>().map_err(|error| PortError::Corrupt {
            what: "generation pointer",
            reason: error.to_string(),
        })?;
        Generation::new(value).map_err(|error| PortError::Corrupt {
            what: "generation pointer",
            reason: error.to_string(),
        })
    }

    async fn apply(&self, transaction: &ProjectionTransaction) -> Result<Applied, PortError> {
        let mut items = Vec::with_capacity(transaction.writes.len());
        for write in &transaction.writes {
            items.push(self.update(write)?);
        }
        let outcome = self
            .client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(Applied::Committed),
            Err(error) if cancelled_by_condition(&error) => {
                // The coverage fence refused, so nothing was written. A
                // redelivered stream record is an expected condition.
                Ok(Applied::AlreadyCovered)
            }
            Err(error) => Err(classify(WHAT, Idempotence::Write, &error)),
        }
    }

    async fn advance_coverage(&self, advance: &CoverageAdvance) -> Result<(), PortError> {
        let key = self
            .keys
            .coverage(
                advance.generation,
                &advance.workspace,
                advance.public_category,
            )
            .map_err(|error| PortError::Corrupt {
                what: "coverage key",
                reason: error.to_string(),
            })?;
        let sort = key.sk.clone().unwrap_or_default();
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .key(attribute::PARTITION, attribute::text(key.pk))
            .key(attribute::SORT, attribute::text(sort))
            .expression_attribute_names("#settled", "settledSequence")
            .expression_attribute_names("#updated", "updatedAt")
            .expression_attribute_values(":settled", attribute::number(advance.settled.get()))
            .expression_attribute_values(
                ":updated",
                attribute::text(advance.updated_at.to_canonical()),
            );
        let mut expression = "SET #settled = :settled, #updated = :updated".to_owned();
        if let Some(through) = advance.settled_through {
            expression.push_str(", #through = :through");
            request = request
                .expression_attribute_names("#through", "settledThrough")
                .expression_attribute_values(":through", attribute::text(through.to_canonical()));
        }
        request
            .update_expression(expression)
            // Settlement only ever moves forward. A late receipt that would move
            // the copy backwards is refused rather than applied, so a customer's
            // coverage never regresses.
            .condition_expression("attribute_not_exists(#settled) OR #settled <= :settled")
            .send()
            .await
            .map_err(|error| {
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
                    )
                }) {
                    return PortError::Conflict { what: "coverage" };
                }
                classify(WHAT, Idempotence::Write, &error)
            })?;
        Ok(())
    }
}

/// Whether a transaction was cancelled by a refused condition.
fn cancelled_by_condition<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        R,
    >,
) -> bool {
    let Some(
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(
            cancelled,
        ),
    ) = error.as_service_error()
    else {
        return false;
    };
    cancelled
        .cancellation_reasons()
        .iter()
        .any(|reason| reason.code() == Some("ConditionalCheckFailed"))
}
