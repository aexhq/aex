//! The pinned observation constants.
//!
//! Every value here is a *registered* ceiling or a named tuning constant from
//! the accepted plan. They live in one module so a second, contradictory copy
//! cannot appear in an adapter — which is exactly how `MAX_OTLP_BYTES = 6 MiB`
//! came to contradict the registered 4 MiB limit in the implementation this
//! replaces.

/// Maximum **encoded** OTLP request body, in bytes.
///
/// 768 KiB, which is what the transport actually permits rather than what the
/// registry once wished for. `regional-otlp` is a Lambda behind an Application
/// Load Balancer, and an ALB caps a Lambda target's request body at 1 MB. It
/// also base64-encodes a binary body on the way in, inflating it by 4/3, so
/// 768 KiB is exactly the largest protobuf batch whose encoded form still fits.
///
/// A ceiling above that is not a larger limit, it is a limit enforced by the
/// wrong component: the load balancer answers 413 before the service sees the
/// request, so the caller learns nothing about which bound it crossed. The
/// earlier 4 MiB — and the 6 MiB it replaced — were both unreachable in
/// production for this reason.
///
/// Raising it again requires moving the deployable off a Lambda target first.
/// See `references/backlog.md`.
pub const OTLP_ENCODED_MAX: usize = 768 * 1024;

/// Maximum **decoded** OTLP payload, in bytes.
pub const OTLP_DECODED_MAX: usize = 16 * 1024 * 1024;

/// Maximum records or data points in one OTLP request.
pub const OTLP_MAX_RECORDS: usize = 2_000;

/// Maximum canonical size of one normalized observation, in bytes.
pub const OBSERVATION_NORMALIZED_MAX: usize = 64 * 1024;

/// Maximum attributes on one resource, scope or record.
pub const OTLP_MAX_ATTRIBUTES: usize = 128;

/// Maximum attribute key length, in bytes.
pub const OTLP_MAX_ATTRIBUTE_KEY_BYTES: usize = 256;

/// Maximum attribute value length, in bytes.
pub const OTLP_MAX_ATTRIBUTE_VALUE_BYTES: usize = 16 * 1024;

/// Maximum elements in an attribute array value.
pub const OTLP_MAX_ARRAY_ELEMENTS: usize = 128;

/// Maximum decompression ratio before the stream is abandoned.
pub const OTLP_MAX_RATIO: u64 = 200;

/// How long a request may wait for a decode-memory permit before the handler
/// answers `503 observability_unavailable`.
pub const OTLP_RESERVE_WAIT_MS: u64 = 50;

/// Bytes of canonical plaintext above which a body moves to `S3`.
///
/// Stopgap, lowered from 32 KiB (2026-08-07 triage): `bodyInline` rides three
/// dense observation GSIs, and the designed per-index copy bound
/// [`OBS_INDEX_INLINE_MAX`] is not enforced anywhere, so every inline byte was
/// stored four times. Holding this at the index bound keeps each GSI copy
/// small while 4-32 KiB bodies take one `S3` put instead. The full fix —
/// a bounded index attribute distinct from the base body — stays backlogged;
/// when it lands this ceiling can return to 32 KiB.
pub const OBS_INLINE_MAX: usize = 4_096;

/// Bytes of `bodyInline` above which the index copy stores an `S3` key instead.
pub const OBS_INDEX_INLINE_MAX: usize = 4_096;

/// Subtracted from `now` when pinning a snapshot, so that everything at or below
/// the snapshot is present in every secondary index by construction.
pub const OBS_INDEX_SETTLE_MS: i64 = 2_000;

/// Admission fails closed when `|now − acceptedAt|` reaches this bound, which is
/// what makes the settle window provably dominate propagation plus clock skew.
pub const OBS_CLOCK_SKEW_MAX_MS: i64 = 1_000;

/// How long a `preparing` receipt survives before the reconciler aborts it.
pub const OBS_PREPARE_TTL_MS: i64 = 15 * 60 * 1_000;

/// Maximum normalized observations packed into one staged page item.
///
/// A page is bounded by **both** this count and [`OBS_PAGE_MAX_BYTES`], and the
/// byte bound is the binding one at the maximum batch. G7 measured that a
/// count-only bound of 100 produces an 819 KiB page at the 2,000-point / 16 MiB
/// maximum, three times the 256 KiB `DynamoDB` item ceiling. The protocol
/// changed here rather than the public 2,000-point limit.
pub const OBS_PAGE_RECORDS: usize = 100;

/// Maximum canonical bytes packed into one staged page item.
///
/// 192 KiB leaves 64 KiB of the 256 KiB item ceiling for the page's own
/// attributes, and it is three times [`OBSERVATION_NORMALIZED_MAX`], so a page
/// always holds at least one observation however large that observation is.
pub const OBS_PAGE_MAX_BYTES: usize = 192 * 1024;

/// Spool chunk attempts before the reconciler escalates to a `pipeline_loss` gap.
pub const OBS_SPOOL_MAX_ATTEMPTS: u32 = 12;

/// Oldest unacknowledged spool chunk age that closes the regional ingress gate.
pub const OBS_SPOOL_MAX_AGE_MS: i64 = 6 * 60 * 60 * 1_000;

/// Minimum dwell in a gate state, so a flapping dependency cannot oscillate it.
pub const OBS_GATE_HYSTERESIS_MS: i64 = 120_000;

/// How often the reconciler re-evaluates the regional ingress gate.
pub const OBS_GATE_INTERVAL_MS: i64 = 10_000;

/// How long index verification may keep failing before it closes the gate.
pub const OBS_INDEX_FAIL_WINDOW_MS: i64 = 15 * 60 * 1_000;

/// Shards over which the workspace series counter is spread.
pub const SERIES_COUNTER_SHARDS: u16 = 256;

/// Default per-workspace metric series ceiling.
pub const SERIES_CARDINALITY_CEILING: u64 = 250_000;

/// Default page size for a public observation query.
pub const QUERY_DEFAULT_LIMIT: u16 = 100;

/// Maximum page size for a public observation query (`query.page`).
pub const QUERY_MAX_LIMIT: u16 = 1_000;

/// Default index items one page may read before it ends early with a cursor.
pub const QUERY_MAX_ITEMS_SCANNED: u32 = 50_000;

/// Default segments one page may open before it ends early with a cursor.
pub const QUERY_MAX_SEGMENTS: u16 = 64;

/// Default bytes one page may read before it ends early with a cursor.
pub const QUERY_MAX_BYTES_READ: u64 = 32 * 1024 * 1024;

/// Maximum raw metric points one aggregation may consume.
pub const METRIC_AGGREGATE_SCAN: u64 = 2_000_000;

/// Maximum theoretical buckets per series in one aggregation.
pub const METRIC_MAX_BUCKETS: u32 = 10_000;

/// Maximum rows one aggregation may return.
pub const METRIC_MAX_ROWS: u32 = 10_000;

/// Maximum group-by fields in one aggregation.
pub const METRIC_MAX_GROUP_FIELDS: usize = 8;

/// Maximum calculations in one aggregation.
pub const METRIC_MAX_CALCULATIONS: usize = 10;

/// The pinned t-digest compression, so a quantile is reproducible.
pub const METRIC_TDIGEST_COMPRESSION: f64 = 100.0;

/// The `DynamoDB` transaction envelope: maximum actions.
pub const DDB_TRANSACT_MAX_ACTIONS: usize = 100;

/// The `DynamoDB` transaction envelope: maximum serialized request bytes.
pub const DDB_TRANSACT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The `DynamoDB` item ceiling.
pub const DDB_ITEM_MAX_BYTES: usize = 256 * 1024;

/// The designed ceiling on transaction C's action count (plan §3.2 step 8).
pub const COMMIT_MAX_ACTIONS: usize = 24;

#[cfg(test)]
mod tests {
    use super::{
        COMMIT_MAX_ACTIONS, DDB_TRANSACT_MAX_ACTIONS, OBS_CLOCK_SKEW_MAX_MS, OBS_INDEX_SETTLE_MS,
        OTLP_ENCODED_MAX,
    };

    #[test]
    fn the_encoded_otlp_ceiling_survives_base64_inside_the_load_balancer_limit() {
        const ALB_LAMBDA_TARGET_REQUEST_MAX: usize = 1024 * 1024;

        assert_eq!(OTLP_ENCODED_MAX, 786_432);
        // Base64 is 4 bytes out per 3 in, rounded up to a 4-byte group. A body
        // at the ceiling must still fit the load balancer's limit after that
        // expansion, or the 413 comes from the ALB and never reaches the
        // service that owns the bound.
        assert!(
            OTLP_ENCODED_MAX.div_ceil(3) * 4 <= ALB_LAMBDA_TARGET_REQUEST_MAX,
            "an encoded body at the ceiling must survive base64 within the \
             ALB's 1 MB Lambda-target request limit"
        );
        assert_ne!(
            OTLP_ENCODED_MAX,
            6 * 1024 * 1024,
            "the 6 MiB constant is not ported"
        );
    }

    #[test]
    fn the_settle_window_strictly_dominates_the_admitted_clock_skew() {
        const {
            assert!(
                OBS_INDEX_SETTLE_MS > OBS_CLOCK_SKEW_MAX_MS,
                "the snapshot contract requires the settle window to dominate skew"
            );
        }
    }

    #[test]
    fn an_inline_body_never_exceeds_what_a_dense_index_copy_may_carry() {
        const {
            assert!(
                super::OBS_INLINE_MAX <= super::OBS_INDEX_INLINE_MAX,
                "bodyInline rides three dense GSIs; until the bounded index \
                 attribute exists, the inline ceiling is the index copy bound"
            );
        }
    }

    #[test]
    fn a_staged_page_always_holds_at_least_one_maximal_observation() {
        const {
            assert!(
                super::OBS_PAGE_MAX_BYTES >= super::OBSERVATION_NORMALIZED_MAX,
                "a page that cannot hold one maximal observation could never make progress"
            );
        }
        const {
            assert!(
                super::OBS_PAGE_MAX_BYTES < super::DDB_ITEM_MAX_BYTES,
                "a page must leave room for its own attributes"
            );
        }
    }

    #[test]
    fn the_commit_ceiling_fits_the_transaction_envelope() {
        const { assert!(COMMIT_MAX_ACTIONS <= DDB_TRANSACT_MAX_ACTIONS) };
    }
}
