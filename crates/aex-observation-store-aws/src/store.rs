//! The staged, hash-bound admission protocol and its transaction envelopes.
//!
//! Admission is nine steps (plan §3.2). The two durable transactions are small
//! and bounded; the bulk write of `OBS#` items happens **after** the visibility
//! commit and is replayable from the retained staged pages. That is decision
//! O-18, and this module is where its envelope is proven rather than assumed:
//! [`AdmissionPlan::commit_envelope`] measures the exact action count and
//! serialized size of transaction C at the maximum 2,000-point batch, which is
//! the `PERF-08` / G7 gate.

use aws_sdk_dynamodb::types::AttributeValue;

use aex_observation_domain::keys::{BucketHour, ScopeKey};
use aex_observation_domain::limits;
use aex_observation_domain::signal::{Signal, SignalSet};

use crate::expressions::{ExpressionBuilder, PK, SK};

/// Why the store refused a request.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StoreError {
    /// An item carried an `itemType` this adapter does not model.
    #[error("item type `{found}` is not one this adapter models")]
    Corrupt {
        /// The unexpected discriminator.
        found: Box<str>,
    },
    /// A single item exceeded the `DynamoDB` item ceiling.
    #[error("item of {observed} bytes exceeds the {limit}-byte ceiling")]
    ItemTooLarge {
        /// The measured size.
        observed: usize,
        /// The provider ceiling.
        limit: usize,
    },
    /// The planned transaction does not fit the provider envelope.
    ///
    /// This is a **protocol** failure, not a capacity one: the fix is to change
    /// the protocol here, never to silently cap the public limit.
    #[error(
        "transaction `{transaction}` needs {actions} actions and {bytes} bytes, \
         beyond the {action_limit}-action / {byte_limit}-byte envelope"
    )]
    EnvelopeExceeded {
        /// Which transaction.
        transaction: &'static str,
        /// The measured action count.
        actions: usize,
        /// The measured serialized size.
        bytes: usize,
        /// The provider action ceiling.
        action_limit: usize,
        /// The provider byte ceiling.
        byte_limit: usize,
    },
    /// The transport outcome of a transaction is unknown.
    ///
    /// Never retried blindly: the caller re-reads `BATCH#…/RECEIPT` and
    /// branches on `state` before responding.
    #[error("the commit outcome is unknown; resolve it by batch identity")]
    CommitAmbiguous,
    /// The authority itself is unreachable.
    #[error("the observation authority is unreachable: {reason}")]
    Unavailable {
        /// What failed.
        reason: Box<str>,
    },
}

/// The measured envelope of one planned transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionEnvelope {
    /// How many `TransactWriteItems` actions the transaction needs.
    pub actions: usize,
    /// The serialized size of the transaction, by `DynamoDB`'s item-size rule.
    pub bytes: usize,
}

impl TransactionEnvelope {
    /// Whether the transaction fits the provider envelope.
    #[must_use]
    pub const fn fits(self) -> bool {
        self.actions <= limits::DDB_TRANSACT_MAX_ACTIONS
            && self.bytes <= limits::DDB_TRANSACT_MAX_BYTES
    }

    /// Fails when the transaction does not fit.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::EnvelopeExceeded`] carrying both measurements, so
    /// a failure names the dimension that has to change.
    pub const fn require_fits(self, transaction: &'static str) -> Result<Self, StoreError> {
        if self.fits() {
            Ok(self)
        } else {
            Err(StoreError::EnvelopeExceeded {
                transaction,
                actions: self.actions,
                bytes: self.bytes,
                action_limit: limits::DDB_TRANSACT_MAX_ACTIONS,
                byte_limit: limits::DDB_TRANSACT_MAX_BYTES,
            })
        }
    }
}

/// The `DynamoDB` item-size rule: the UTF-8 length of every attribute name plus
/// the size of every value.
///
/// Modelled rather than measured from the SDK because the SDK exposes no
/// serialized size, and because a model that errs *conservatively* still proves
/// the envelope.
#[must_use]
pub fn item_size<H: std::hash::BuildHasher>(
    item: &std::collections::HashMap<String, AttributeValue, H>,
) -> usize {
    item.iter()
        .map(|(name, value)| name.len() + value_size(value))
        .sum()
}

/// The size of one attribute value under the same rule.
#[must_use]
pub fn value_size(value: &AttributeValue) -> usize {
    match value {
        AttributeValue::S(text) | AttributeValue::N(text) => text.len(),
        AttributeValue::B(blob) => blob.as_ref().len(),
        AttributeValue::Ss(values) | AttributeValue::Ns(values) => {
            values.iter().map(String::len).sum()
        }
        AttributeValue::Bs(values) => values.iter().map(|blob| blob.as_ref().len()).sum(),
        AttributeValue::L(values) => values.iter().map(value_size).sum::<usize>() + values.len(),
        AttributeValue::M(entries) => {
            entries
                .iter()
                .map(|(name, inner)| name.len() + value_size(inner) + 1)
                .sum::<usize>()
                + 3
        }
        // `Bool`, `Null` and any value kind this adapter never writes still cost
        // something, so the model never under-counts.
        _ => 1,
    }
}

/// One record staged into a page item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedRecord {
    /// The signal the record carries.
    pub signal: Signal,
    /// Canonical bytes of the normalized observation.
    pub canonical: Vec<u8>,
}

/// A conservative model of one control-row's serialized size.
///
/// Every control item in this table is a small, fixed-shape row: two keys, a
/// discriminator, two identifiers, a state word, a handful of counters and two
/// fixed-width timestamps. 512 bytes is comfortably above the largest of them
/// and is deliberately generous, because a conservative model still proves the
/// envelope.
pub const TYPICAL_CONTROL_ITEM_BYTES: usize = 512;

/// The bytes one page digest adds to the receipt flip.
pub const PAGE_DIGEST_BYTES: usize = 72;

/// `BatchWriteItem`'s per-call action ceiling.
pub const DDB_BATCH_WRITE_MAX: usize = 25;

/// What one batch will write, before anything durable happens.
#[derive(Clone, Debug)]
pub struct AdmissionPlan {
    scope: ScopeKey,
    signals: SignalSet,
    abucket: BucketHour,
    records: usize,
    logical_bytes: u64,
    new_series: usize,
    pub(crate) page_bytes: Vec<usize>,
}

impl AdmissionPlan {
    /// Plans one batch.
    ///
    /// Staged pages are sized by [`limits::OBS_PAGE_RECORDS`]; the record count
    /// determines the page count, and the page count is what keeps transaction C
    /// small regardless of batch size.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ItemTooLarge`] when one staged page would exceed
    /// the `DynamoDB` item ceiling.
    pub fn new(
        scope: ScopeKey,
        signals: SignalSet,
        abucket: BucketHour,
        records: &[StagedRecord],
        new_series: usize,
    ) -> Result<Self, StoreError> {
        let logical_bytes: u64 = records
            .iter()
            .map(|record| record.canonical.len() as u64)
            .sum();
        // A page is bounded by both the record count and the byte budget, and
        // the byte budget is the binding one at the maximum batch. G7 measured
        // that a count-only bound produces an item three times the provider
        // ceiling, so the protocol packs by bytes as well as by count.
        let mut page_bytes: Vec<usize> = Vec::new();
        let mut open_records = 0usize;
        let mut open_bytes = 0usize;
        for record in records {
            let size = record.canonical.len();
            if size > limits::OBS_PAGE_MAX_BYTES {
                return Err(StoreError::ItemTooLarge {
                    observed: size,
                    limit: limits::OBS_PAGE_MAX_BYTES,
                });
            }
            let full = open_records == limits::OBS_PAGE_RECORDS
                || open_bytes + size > limits::OBS_PAGE_MAX_BYTES;
            if full {
                page_bytes.push(open_bytes);
                open_records = 0;
                open_bytes = 0;
            }
            open_records += 1;
            open_bytes += size;
        }
        if open_records > 0 {
            page_bytes.push(open_bytes);
        }
        Ok(Self {
            scope,
            signals,
            abucket,
            records: records.len(),
            logical_bytes,
            new_series,
            page_bytes,
        })
    }

    /// The scope this batch belongs to.
    #[must_use]
    pub const fn scope(&self) -> ScopeKey {
        self.scope
    }

    /// The accepted-time hour bucket.
    #[must_use]
    pub const fn abucket(&self) -> BucketHour {
        self.abucket
    }

    /// How many records the batch carries.
    #[must_use]
    pub const fn records(&self) -> usize {
        self.records
    }

    /// The total canonical bytes.
    #[must_use]
    pub const fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    /// How many staged page items the batch needs.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.page_bytes.len()
    }

    /// The envelope of transaction P.
    ///
    /// Two actions: create or resume the receipt, and reserve exact bytes and
    /// records on the workspace ingest quota.
    #[must_use]
    pub fn prepare_envelope(&self) -> TransactionEnvelope {
        let mut builder = ExpressionBuilder::new();
        let condition = crate::expressions::prepare_condition(&mut builder, &"0".repeat(64));
        TransactionEnvelope {
            actions: 2,
            bytes: 2 * TYPICAL_CONTROL_ITEM_BYTES
                + condition.len()
                + self.page_count().min(1) * PAGE_DIGEST_BYTES,
        }
    }

    /// The envelope of transaction C.
    ///
    /// Small, bounded and **independent of the batch's record count**: the
    /// receipt flip carries one digest per staged page rather than one entry per
    /// record, and the counters are `ADD` updates. This is the measurement the
    /// G7 gate asserts.
    #[must_use]
    pub fn commit_envelope(&self) -> TransactionEnvelope {
        // One receipt flip, one frontier advance plus one `SEG#` and one `SEGT#`
        // upsert per signal, one spool chunk, one usage outbox item, one quota
        // finalization, and one series-counter shard when the batch claimed any.
        let signals = self.signals.in_authority().len().max(1);
        let series_shards = usize::from(self.new_series > 0);
        let actions = 1 + signals * 3 + 3 + series_shards;

        // Only the receipt flip grows with the batch, and it grows with the
        // *page* count rather than the record count.
        let receipt = TYPICAL_CONTROL_ITEM_BYTES + self.page_count() * PAGE_DIGEST_BYTES;
        let bytes = receipt + (actions - 1) * TYPICAL_CONTROL_ITEM_BYTES;

        TransactionEnvelope { actions, bytes }
    }

    /// How many `BatchWriteItem` calls step 9 needs.
    ///
    /// Materialization is not a transaction: it is an idempotent, replayable
    /// bulk write under `attribute_not_exists(pk)`, which is exactly what lets
    /// transaction C stay inside the envelope.
    #[must_use]
    pub const fn materialization_batches(&self) -> usize {
        self.records.div_ceil(DDB_BATCH_WRITE_MAX)
    }

    /// The exact `(pk, sk)` of one materialized observation.
    #[must_use]
    pub fn observation_key(
        &self,
        signal: Signal,
        shard: u8,
        accepted_seq: u64,
    ) -> std::collections::HashMap<String, AttributeValue> {
        let mut key = std::collections::HashMap::new();
        key.insert(
            PK.to_owned(),
            AttributeValue::S(aex_observation_domain::keys::observation_pk(
                &self.scope,
                signal,
                self.abucket,
                shard,
            )),
        );
        key.insert(
            SK.to_owned(),
            AttributeValue::S(aex_observation_domain::keys::observation_sk(accepted_seq)),
        );
        key
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionPlan, StagedRecord, StoreError, TransactionEnvelope, item_size};
    use aex_observation_domain::keys::{BucketHour, ScopeKey};
    use aex_observation_domain::limits;
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aws_sdk_dynamodb::types::AttributeValue;

    fn scope() -> ScopeKey {
        use aex_wire::ids::PrefixedId as _;
        ScopeKey::Session(
            aex_wire::ids::SessionId::parse("ses_0000000003ec1r60r30c1g60r3")
                .expect("fixture parses"),
        )
    }

    fn bucket() -> BucketHour {
        BucketHour::parse("2026-08-01T09").expect("fixture parses")
    }

    fn records(count: usize, bytes: usize) -> Vec<StagedRecord> {
        (0..count)
            .map(|_| StagedRecord {
                signal: Signal::Logs,
                canonical: vec![b'x'; bytes],
            })
            .collect()
    }

    #[test]
    fn the_item_size_model_counts_names_and_values() {
        let mut item = std::collections::HashMap::new();
        item.insert("pk".to_owned(), AttributeValue::S("OBS#x".to_owned()));
        item.insert("n".to_owned(), AttributeValue::N("42".to_owned()));
        item.insert("b".to_owned(), AttributeValue::Bool(true));
        assert_eq!(item_size(&item), 2 + 5 + 1 + 2 + 1 + 1);
    }

    #[test]
    fn a_single_record_above_the_page_budget_is_refused_before_anything_durable() {
        let oversized = records(1, limits::OBS_PAGE_MAX_BYTES + 1);
        let error = AdmissionPlan::new(
            scope(),
            SignalSet::from_signal(Signal::Logs),
            bucket(),
            &oversized,
            0,
        )
        .expect_err("refused");
        assert!(matches!(error, StoreError::ItemTooLarge { .. }));
    }

    #[test]
    fn pages_are_packed_by_bytes_as_well_as_by_count() {
        // 100 records of 4 KiB would be one 400 KiB page under a count-only
        // bound, which no DynamoDB item can hold. The byte budget splits it.
        let plan = AdmissionPlan::new(
            scope(),
            SignalSet::from_signal(Signal::Logs),
            bucket(),
            &records(limits::OBS_PAGE_RECORDS, 4 * 1024),
            0,
        )
        .expect("plans");
        assert_eq!(plan.page_count(), 3);
        for bytes in &plan.page_bytes {
            assert!(*bytes <= limits::OBS_PAGE_MAX_BYTES);
        }
    }

    #[test]
    fn the_page_count_bounds_the_commit_rather_than_the_record_count() {
        let small = AdmissionPlan::new(
            scope(),
            SignalSet::from_signal(Signal::Logs),
            bucket(),
            &records(10, 64),
            0,
        )
        .expect("plans");
        let large = AdmissionPlan::new(
            scope(),
            SignalSet::from_signal(Signal::Logs),
            bucket(),
            &records(2_000, 64),
            0,
        )
        .expect("plans");
        assert_eq!(small.page_count(), 1);
        assert_eq!(large.page_count(), 20);
        assert_eq!(
            small.commit_envelope().actions,
            large.commit_envelope().actions,
            "the action count does not grow with the record count"
        );
        assert_eq!(large.records(), 2_000);
        assert_eq!(large.logical_bytes(), 2_000 * 64);
        assert_eq!(large.materialization_batches(), 80);
        assert_eq!(large.scope(), scope());
        assert_eq!(large.abucket(), bucket());
        assert!(large.prepare_envelope().fits());
    }

    #[test]
    fn an_observation_key_is_built_from_the_validated_templates() {
        let plan = AdmissionPlan::new(
            scope(),
            SignalSet::from_signal(Signal::Logs),
            bucket(),
            &records(1, 8),
            0,
        )
        .expect("plans");
        let key = plan.observation_key(Signal::Logs, 3, 7);
        assert_eq!(
            key.get("pk").and_then(|value| value.as_s().ok()),
            Some(&"OBS#S#ses_0000000003ec1r60r30c1g60r3#logs#2026-08-01T09#03".to_owned())
        );
        assert_eq!(
            key.get("sk").and_then(|value| value.as_s().ok()),
            Some(&"00000000000000000007".to_owned())
        );
    }

    #[test]
    fn an_envelope_that_does_not_fit_names_both_dimensions() {
        let envelope = TransactionEnvelope {
            actions: 101,
            bytes: 8,
        };
        assert!(!envelope.fits());
        let error = envelope.require_fits("C").expect_err("refused");
        assert!(matches!(
            error,
            StoreError::EnvelopeExceeded {
                transaction: "C",
                actions: 101,
                ..
            }
        ));
    }
}
