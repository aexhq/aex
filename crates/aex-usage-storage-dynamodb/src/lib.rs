//! `aex-usage-storage-dynamodb` is the `usage-storage-authority` adapter: table expressions, the row codec, the admission transaction and the frontier compare-and-set for `storage.byte_min.v1` facts, plus the rating-queue producer and this category's settlement receipt consumer.

use aex_usage_domain::meter::Category;

/// The one authority this crate can address.
pub const CATEGORY: Category = Category::Storage;
/// The environment variable naming the authority table.
pub const TABLE_ENV: &str = "AEX_USAGE_STORAGE_TABLE";
/// The environment variable naming this category's receipt queue.
pub const RECEIPT_QUEUE_ENV: &str = "AEX_USAGE_RECEIPT_STORAGE_QUEUE";
/// The environment variable naming the shared rating queue.
pub const RATING_QUEUE_ENV: &str = "AEX_USAGE_RATING_QUEUE";

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct StorageBinding;

impl aex_usage_authority_dynamodb::AuthorityBinding for StorageBinding {
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
/// Storage-bound row and transaction expressions.
pub mod expressions {
    pub use aex_usage_authority_dynamodb::expressions::*;
    /// Expression builder that can address only the storage authority.
    pub type StorageAuthority = Authority<crate::StorageBinding>;
}
/// Storage-bound authority store.
pub mod store {
    pub use aex_usage_authority_dynamodb::store::{OUTBOX_SHARDS, due_partition, shard_of};
    /// `DynamoDB` store that can address only the storage authority.
    pub type StorageStore = aex_usage_authority_dynamodb::store::Store<crate::StorageBinding>;
}
/// Strict storage-authority row codec.
#[allow(clippy::implicit_hasher, clippy::missing_errors_doc)]
pub mod codec {
    pub use aex_usage_authority_dynamodb::codec::{
        ClaimRow, DecodeError, OutboxRow, StoredReceipt, decode_claim, decode_receipt,
    };
    use aws_sdk_dynamodb::types::AttributeValue;
    use std::collections::HashMap;

    /// Decodes and revalidates one storage fact row.
    pub fn decode_fact(
        map: &HashMap<String, AttributeValue>,
    ) -> Result<aex_usage_domain::fact::UsageFact, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_fact::<crate::StorageBinding>(map)
    }
    /// Decodes one storage frontier row.
    pub fn decode_frontier(
        map: &HashMap<String, AttributeValue>,
    ) -> Result<aex_usage_domain::frontier::Frontier, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_frontier::<crate::StorageBinding>(map)
    }
    /// Decodes one storage outbox row from the base table.
    pub fn decode_outbox(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_outbox::<crate::StorageBinding>(map)
    }
    /// Decodes one storage outbox row from the due index.
    pub fn decode_due(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        aex_usage_authority_dynamodb::codec::decode_due::<crate::StorageBinding>(map)
    }
}
/// Storage-authority stream and receipt envelope decoder.
#[allow(clippy::missing_errors_doc)]
pub mod stream {
    pub use aex_usage_authority_dynamodb::stream::{
        INSERT, ReceiptEnvelope, ReceiptError, StreamError,
    };

    /// Decodes a storage authority `DynamoDB` stream batch.
    pub fn stream_records(
        event: &serde_json::Value,
    ) -> Result<Vec<aex_usage_app::worker::StreamRecord>, StreamError> {
        aex_usage_authority_dynamodb::stream::stream_records::<crate::StorageBinding>(event)
    }
    /// Decodes a storage settlement receipt queue batch.
    pub fn receipt_records(
        event: &serde_json::Value,
    ) -> Result<Vec<aex_usage_app::worker::ReceiptRecord>, ReceiptError> {
        aex_usage_authority_dynamodb::stream::receipt_records::<crate::StorageBinding>(event)
    }
}

pub use attribute::{Row, RowError};
pub use clock::SystemClock;
pub use codec::{ClaimRow, DecodeError, OutboxRow, StoredReceipt};
pub use expressions::{
    AdmissionTransaction, FACT_BODY, FRONTIER_CAS, ReceiptRow, StorageAuthority, StoreError,
    TransactItem, WRITE_ONCE,
};
pub use projection::QueryProjection;
pub use queue::SettlementQueue;
pub use store::StorageStore;
pub use stream::{ReceiptEnvelope, ReceiptError, StreamError};

#[cfg(test)]
mod tests {
    #[test]
    fn public_authority_type_is_bound_to_the_facade_category() {
        assert_eq!(crate::StorageAuthority::new().category(), crate::CATEGORY);
    }
}
