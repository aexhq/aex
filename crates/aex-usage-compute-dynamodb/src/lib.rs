//! `aex-usage-compute-dynamodb` is the `usage-compute-authority` adapter: table expressions, the row codec, the admission transaction and the frontier compare-and-set for `compute.millicpu_ms.v1` and `memory.byte_ms.v1` facts, plus the rating-queue producer and this category's settlement receipt consumer.
//!
//! This crate retains the compute table binding and public API. The strict row,
//! expression, transaction, stream, queue, and projection mechanisms live in
//! `aex-usage-authority-dynamodb` and are selected here with one category marker.

use aex_usage_domain::meter::Category;

/// The one authority this crate can address.
pub const CATEGORY: Category = Category::Compute;
/// The environment variable naming the authority table.
pub const TABLE_ENV: &str = "AEX_USAGE_COMPUTE_TABLE";
/// The environment variable naming this category's settlement receipt queue.
pub const RECEIPT_QUEUE_ENV: &str = "AEX_USAGE_RECEIPT_COMPUTE_QUEUE";
/// The environment variable naming the shared central rating queue.
pub const RATING_QUEUE_ENV: &str = "AEX_USAGE_RATING_QUEUE";

/// Compile-time binding between this facade and the compute authority.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct ComputeBinding;

impl aex_usage_authority_dynamodb::AuthorityBinding for ComputeBinding {
    const CATEGORY: Category = CATEGORY;
}

pub use aex_usage_authority_dynamodb::gsi;

/// Strict `DynamoDB` attribute conversion and row access.
pub mod attribute {
    pub use aex_usage_authority_dynamodb::attribute::*;
}
/// Authority wall clock.
pub mod clock {
    pub use aex_usage_authority_dynamodb::clock::*;
}
/// AWS failure classification.
pub mod fault {
    pub use aex_usage_authority_dynamodb::fault::*;
}
/// Reserved outbox surface retained by the typed facade.
pub mod outbox {}
/// Shared query-projection adapter.
pub mod projection {
    pub use aex_usage_authority_dynamodb::projection::*;
}
/// Central rating queue adapter.
pub mod queue {
    pub use aex_usage_authority_dynamodb::queue::*;
}
/// Compute-bound row and transaction expressions.
pub mod expressions {
    pub use aex_usage_authority_dynamodb::expressions::*;
    /// Expression builder that can address only the compute authority.
    pub type ComputeAuthority = Authority<crate::ComputeBinding>;
}
/// Compute-bound authority store.
pub mod store {
    pub use aex_usage_authority_dynamodb::store::{OUTBOX_SHARDS, due_partition, shard_of};
    /// `DynamoDB` store that can address only the compute authority.
    pub type ComputeStore = aex_usage_authority_dynamodb::store::Store<crate::ComputeBinding>;
}
/// Strict compute-authority row codec.
#[allow(clippy::implicit_hasher, clippy::missing_errors_doc)]
pub mod codec {
    pub use aex_usage_authority_dynamodb::codec::{
        ClaimRow, DecodeError, OutboxRow, StoredReceipt, decode_claim, decode_receipt,
    };
    use aws_sdk_dynamodb::types::AttributeValue;
    use std::collections::HashMap;

    /// Decodes and revalidates one compute fact row.
    pub fn decode_fact(
        map: &HashMap<String, AttributeValue>,
    ) -> Result<aex_usage_domain::fact::UsageFact, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_fact::<crate::ComputeBinding>(map)
    }
    /// Decodes one compute frontier row.
    pub fn decode_frontier(
        map: &HashMap<String, AttributeValue>,
    ) -> Result<aex_usage_domain::frontier::Frontier, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_frontier::<crate::ComputeBinding>(map)
    }
    /// Decodes one compute outbox row from the base table.
    pub fn decode_outbox(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_outbox::<crate::ComputeBinding>(map)
    }
    /// Decodes one compute outbox row from the due index.
    pub fn decode_due(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_due::<crate::ComputeBinding>(map)
    }
}
/// Compute-authority stream and receipt envelope decoder.
#[allow(clippy::missing_errors_doc)]
pub mod stream {
    pub use aex_usage_authority_dynamodb::stream::{
        INSERT, ReceiptEnvelope, ReceiptError, StreamError,
    };

    /// Decodes a compute authority `DynamoDB` stream batch.
    pub fn stream_records(
        event: &serde_json::Value,
    ) -> Result<Vec<aex_usage_app::worker::StreamRecord>, StreamError> {
        aex_usage_authority_dynamodb::stream::stream_records::<crate::ComputeBinding>(event)
    }
    /// Decodes a compute settlement receipt queue batch.
    pub fn receipt_records(
        event: &serde_json::Value,
    ) -> Result<Vec<aex_usage_app::worker::ReceiptRecord>, ReceiptError> {
        aex_usage_authority_dynamodb::stream::receipt_records::<crate::ComputeBinding>(event)
    }
}

pub use attribute::{Row, RowError};
pub use clock::SystemClock;
pub use codec::{ClaimRow, DecodeError, OutboxRow, StoredReceipt};
pub use expressions::{
    AdmissionTransaction, ComputeAuthority, FACT_BODY, FRONTIER_CAS, ReceiptRow, StoreError,
    TransactItem, WRITE_ONCE,
};
pub use projection::QueryProjection;
pub use queue::SettlementQueue;
pub use store::ComputeStore;
pub use stream::{ReceiptEnvelope, ReceiptError, StreamError};

#[cfg(test)]
mod tests {
    #[test]
    fn public_authority_type_is_bound_to_the_facade_category() {
        assert_eq!(crate::ComputeAuthority::new().category(), crate::CATEGORY);
    }
}
