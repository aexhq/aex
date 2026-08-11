//! Bounded, ordered staging of immutable observation bodies.
//!
//! Staging is the one admission step whose cost scales with the caller's record
//! count. A legal maximum-sized batch of bodies just above the inline threshold
//! is hundreds of `S3` writes, and issuing them one after another can burn the
//! whole request timeout before transaction C is even attempted. They are
//! therefore issued concurrently — but under two explicit bounds, and never as a
//! whole-batch fan-out: an admission edge whose peak footprint is chosen by the
//! customer's record count has no bound at all.
//!
//! Three properties are structural rather than conventional:
//!
//! - **Order survives concurrency.** Materialization pairs the *n*-th placement
//!   with the *n*-th observation, so results are collected by input position and
//!   never by completion order.
//! - **A partial placement vector never escapes.** The earliest failure by input
//!   position is returned whole, so there is no half-staged vector for a caller
//!   to commit or materialize.
//! - **Both bounds are read, not restated.** The aggregate in-flight byte budget
//!   is the registry's own decoded-request ceiling.

use std::future::Future;

use aex_observation_domain::canonical::sha256_hex;
use aex_observation_domain::keys::ScopeKey;
use aex_observation_domain::limits;
use aex_observation_store_dynamodb::store::StoreError;
use aex_otlp_admission::OtlpLimits;
use async_trait::async_trait;
use futures::StreamExt as _;
use tokio::sync::Semaphore;

use crate::authority::{AuthorityError, PreparedObservation};

/// The `S3` object prefix immutable observation bodies live under.
pub const BODY_PREFIX: &str = "observations";

/// How many body writes one batch may hold open at once.
///
/// It converts this deployable's own tail-latency exposure: the request timeout
/// the release registers is the real constraint, and a serial chain of hundreds
/// of writes cannot be shown to fit inside it. Eight keeps the shared client's
/// connection footprint an order of magnitude below the reserved concurrency the
/// whole region is sized against while removing the serial chain. It bounds
/// *writes*, not observations — an inline body never reaches the sink.
pub const MAX_CONCURRENT_BODY_PUTS: usize = 8;

/// Bytes one in-flight staging permit stands for.
///
/// The weighted budget is accounted in whole kibibytes, rounded up per body, so
/// a batch of small bodies can never round its way past the ceiling.
pub const STAGING_BYTE_QUANTUM: usize = 1024;

/// The aggregate canonical bytes one batch may hold in flight while staging.
///
/// Read from the registry rather than chosen here: a request can never have more
/// canonical bytes open against `S3` than the whole request was allowed to
/// decode into in the first place.
pub const STAGING_BYTE_BUDGET: usize = OtlpLimits::REGISTERED.decoded_max;

/// Where one observation's canonical body was placed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BodyPlacement {
    /// Small enough to live on the item.
    Inline,
    /// Content-addressed in the observation bucket.
    Object {
        /// The object key.
        key: String,
        /// The content digest.
        digest: String,
    },
}

/// What one content-addressed write found at its key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PutOutcome {
    /// The key was free and now holds these exact bytes.
    Created,
    /// The key was already occupied by an object of this exact digest.
    AlreadyPresent,
}

/// The one immutable-body write staging performs.
///
/// A port rather than a direct client call, because the bounds this module
/// exists to hold are properties of *scheduling*, and a bound that can only be
/// demonstrated against a real bucket is a bound nothing checks. Every case in
/// this module drives the real stager against a sink that records what it was
/// asked to do and when.
#[async_trait]
pub trait BodySink: Send + Sync {
    /// Writes one content-addressed body without overwriting an occupied key.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when the write fails for any reason
    /// other than the key already holding this digest.
    async fn put_body(&self, key: &str, canonical: &[u8]) -> Result<PutOutcome, AuthorityError>;
}

/// The `S3` implementation of [`BodySink`].
#[derive(Clone, Debug)]
pub struct S3BodySink<'a> {
    s3: &'a aws_sdk_s3::Client,
    bucket: &'a str,
}

impl<'a> S3BodySink<'a> {
    /// Binds the sink to the regional observation bucket.
    #[must_use]
    pub const fn new(s3: &'a aws_sdk_s3::Client, bucket: &'a str) -> Self {
        Self { s3, bucket }
    }
}

#[async_trait]
impl BodySink for S3BodySink<'_> {
    async fn put_body(&self, key: &str, canonical: &[u8]) -> Result<PutOutcome, AuthorityError> {
        let outcome = self
            .s3
            .put_object()
            .bucket(self.bucket)
            .key(key)
            .if_none_match("*")
            .content_length(i64::try_from(canonical.len()).unwrap_or(i64::MAX))
            .body(canonical.to_vec().into())
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(PutOutcome::Created),
            Err(error) => {
                // A `412` means the content-addressed object already exists.
                // The key *is* the digest, so an unequal body cannot be
                // addressed here at all.
                let service = error
                    .raw_response()
                    .map(|response| response.status().as_u16());
                if service == Some(412) {
                    Ok(PutOutcome::AlreadyPresent)
                } else {
                    Err(AuthorityError::provider("PutObject", error))
                }
            }
        }
    }
}

/// Stages a batch of immutable bodies under both bounds, in observation order.
pub struct BodyStager<'a> {
    sink: &'a dyn BodySink,
    scope: ScopeKey,
    byte_permits: Semaphore,
    total_byte_permits: usize,
}

impl<'a> BodyStager<'a> {
    /// Binds a stager to its sink and to the byte budget it may hold in flight.
    ///
    /// The budget is supplied rather than defaulted, for the same reason no
    /// resource identifier in this deployable is: a bound nobody chose is a
    /// bound nobody owns.
    #[must_use]
    pub fn new(sink: &'a dyn BodySink, scope: ScopeKey, byte_budget: usize) -> Self {
        // Rounded down, so the permits that exist can never stand for more bytes
        // than the budget allows.
        let total_byte_permits = byte_budget / STAGING_BYTE_QUANTUM;
        Self {
            sink,
            scope,
            byte_permits: Semaphore::new(total_byte_permits),
            total_byte_permits,
        }
    }

    /// Stages every body of one batch, in input order, under both bounds.
    ///
    /// # Errors
    ///
    /// Returns the failure of the earliest observation by input position. There
    /// is deliberately no partial result: a batch that could not be staged whole
    /// has nothing for the caller to commit or materialize.
    pub async fn stage_all(
        &self,
        observations: &[PreparedObservation],
    ) -> Result<Vec<BodyPlacement>, AuthorityError> {
        // The operations are built before the batch starts, not while it runs:
        // an unpolled future has issued nothing, and building them here keeps the
        // stream's own type free of a borrow-returning closure the `Send` check
        // cannot reason about (rust-lang/rust#102211).
        let staging: Vec<_> = observations
            .iter()
            .map(|observation| self.stage_one(observation))
            .collect();
        settle_bounded_ordered(staging, MAX_CONCURRENT_BODY_PUTS).await
    }

    /// The effective in-flight byte budget, after quantization.
    #[must_use]
    pub const fn byte_budget(&self) -> usize {
        self.total_byte_permits * STAGING_BYTE_QUANTUM
    }

    /// Stages one immutable body, inline or through the sink.
    async fn stage_one(
        &self,
        observation: &PreparedObservation,
    ) -> Result<BodyPlacement, AuthorityError> {
        if observation.canonical.len() <= limits::OBS_INLINE_MAX {
            // An inline body is carried on the observation item itself. It
            // performs no write, so it takes no byte permit and holds no write
            // slot beyond deciding that.
            return Ok(BodyPlacement::Inline);
        }
        let weight = self.byte_permits_for(observation.canonical.len())?;
        // Held across the whole write: the budget bounds bytes *in flight*, not
        // bytes admitted.
        let _permit = self
            .byte_permits
            .acquire_many(weight)
            .await
            .map_err(|error| AuthorityError::provider("StageBody", error))?;
        let digest = sha256_hex(&observation.canonical);
        let key = body_object_key(self.scope, &digest);
        match self.sink.put_body(&key, &observation.canonical).await? {
            // Both outcomes are the same placement. The key is the digest, so an
            // object that is already present holds these exact bytes, and a
            // re-presented batch places exactly where its first attempt placed.
            PutOutcome::Created | PutOutcome::AlreadyPresent => {
                Ok(BodyPlacement::Object { key, digest })
            }
        }
    }

    /// The byte permits one body costs while its write is open.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ItemTooLarge`] when one body alone is larger than
    /// the whole in-flight budget. Waiting for permits that can never be issued
    /// is a hang, not a bound.
    fn byte_permits_for(&self, bytes: usize) -> Result<u32, AuthorityError> {
        let weight = bytes.div_ceil(STAGING_BYTE_QUANTUM);
        u32::try_from(weight)
            .ok()
            .filter(|_| weight <= self.total_byte_permits)
            .ok_or(AuthorityError::Store(StoreError::ItemTooLarge {
                observed: bytes,
                limit: self.byte_budget(),
            }))
    }
}

/// The content-addressed key one body is written to.
///
/// The digest is both the name and the proof: two batches carrying the same
/// bytes address the same object, which is what makes a re-presented body a
/// precondition failure rather than a second copy.
fn body_object_key(scope: ScopeKey, digest: &str) -> String {
    let prefix = scope_body_prefix(scope);
    format!("{prefix}/{}/{}/{digest}", &digest[0..2], &digest[2..4])
}

/// Exact object prefix owned by one scope.
///
/// Session bodies never share an object key with another session, even when
/// their canonical bytes are equal. That makes exact-session deletion safe:
/// deleting its prefix cannot erase a body another live scope still names.
#[must_use]
pub fn scope_body_prefix(scope: ScopeKey) -> String {
    let owner = match scope {
        ScopeKey::Session { session, .. } => format!("sessions/{session}"),
        ScopeKey::Workspace(_) => "workspace".to_owned(),
    };
    format!("{BODY_PREFIX}/{}/{owner}", scope.workspace())
}

/// Settles `operations` with at most `max_in_flight` open, in input order.
///
/// At most `max_in_flight` operations are ever *polled*, which is what bounds
/// the batch. A caller that hands over a whole batch of already-constructed
/// futures is still bounded, because an unpolled future has issued nothing:
/// there is no whole-batch submission and no unbounded fan-out however many
/// records the caller sent.
///
/// # Errors
///
/// Returns the failure of the earliest operation by input position, never the
/// first one to answer. A provider that fails the tail of a batch faster than
/// its head must not be able to choose which error the caller sees.
///
/// # Panics
///
/// Panics when `max_in_flight` is zero, which is a bound that admits nothing.
pub async fn settle_bounded_ordered<T, I>(
    operations: I,
    max_in_flight: usize,
) -> Result<Vec<T>, AuthorityError>
where
    I: IntoIterator,
    I::Item: Future<Output = Result<T, AuthorityError>>,
{
    let operations = operations.into_iter();
    let mut settled = Vec::with_capacity(operations.size_hint().0);
    let mut open = futures::stream::iter(operations).buffered(max_in_flight);
    while let Some(outcome) = open.next().await {
        // Returning here drops the stream, which both stops the batch launching
        // anything further and cancels what is still open. Cancelling a
        // content-addressed write is safe by construction: nothing staged is
        // reachable until transaction C publishes the accepted range, and this
        // batch will now never reach it.
        settled.push(outcome?);
    }
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use aex_observation_domain::canonical::CanonicalValue;
    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_domain::limits;
    use aex_observation_domain::signal::Signal;
    use aex_otlp_admission::OtlpLimits;
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        BODY_PREFIX, BodyPlacement, BodySink, BodyStager, MAX_CONCURRENT_BODY_PUTS, PutOutcome,
        STAGING_BYTE_BUDGET, STAGING_BYTE_QUANTUM, body_object_key, settle_bounded_ordered,
    };
    use crate::authority::{AuthorityError, PreparedObservation};

    /// One body above the inline threshold, tagged with its batch position.
    fn object_body(position: usize) -> PreparedObservation {
        let mut canonical = vec![0_u8; limits::OBS_INLINE_MAX + 1];
        canonical[..8].copy_from_slice(&(position as u64).to_be_bytes());
        prepared(canonical)
    }

    /// One body at or below the inline threshold.
    fn inline_body(bytes: usize) -> PreparedObservation {
        assert!(
            bytes <= limits::OBS_INLINE_MAX,
            "the fixture must stay inline"
        );
        prepared(vec![7_u8; bytes])
    }

    fn prepared(canonical: Vec<u8>) -> PreparedObservation {
        PreparedObservation {
            signal: Signal::Logs,
            time: Timestamp::from_unix_millis(1).expect("fixture instant"),
            canonical,
            body: CanonicalValue::Null,
            attr_digest: "0".repeat(64),
            trace_id: None,
            span_id: None,
            metric_name: None,
            series_hash: None,
        }
    }

    /// Reads the batch position back out of a staged body.
    fn position_of(canonical: &[u8]) -> usize {
        let mut tag = [0_u8; 8];
        tag.copy_from_slice(&canonical[..8]);
        usize::try_from(u64::from_be_bytes(tag)).expect("a fixture position")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(2, [2; 10]))
    }

    fn scope() -> ScopeKey {
        ScopeKey::Workspace(workspace())
    }

    fn session_scope(tag: u8) -> ScopeKey {
        ScopeKey::Session {
            workspace: workspace(),
            session: SessionId::from_uuid7(Uuid7::compose(2, [tag; 10])),
        }
    }

    /// What the stager asked the sink to do, and how much of it overlapped.
    #[derive(Debug, Default)]
    struct SinkLedger {
        launched: Vec<usize>,
        completed: Vec<usize>,
        open: usize,
        peak_open: usize,
        open_bytes: usize,
        peak_open_bytes: usize,
    }

    /// A sink that records every write instead of performing one.
    #[derive(Debug)]
    struct RecordingSink {
        /// How long the write of each tagged position takes.
        delay_ms: Vec<u64>,
        /// The tagged positions whose write fails.
        fails_at: Vec<usize>,
        /// What a successful write reports.
        outcome: PutOutcome,
        ledger: Mutex<SinkLedger>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                delay_ms: Vec::new(),
                fails_at: Vec::new(),
                outcome: PutOutcome::Created,
                ledger: Mutex::new(SinkLedger::default()),
            }
        }

        fn with_delays(mut self, delay_ms: Vec<u64>) -> Self {
            self.delay_ms = delay_ms;
            self
        }

        fn failing_at(mut self, positions: &[usize]) -> Self {
            self.fails_at = positions.to_vec();
            self
        }

        fn reporting(mut self, outcome: PutOutcome) -> Self {
            self.outcome = outcome;
            self
        }

        fn ledger(&self) -> std::sync::MutexGuard<'_, SinkLedger> {
            self.ledger.lock().expect("the ledger is never poisoned")
        }
    }

    #[async_trait::async_trait]
    impl BodySink for RecordingSink {
        async fn put_body(
            &self,
            _key: &str,
            canonical: &[u8],
        ) -> Result<PutOutcome, AuthorityError> {
            let position = position_of(canonical);
            {
                let mut ledger = self.ledger();
                ledger.launched.push(position);
                ledger.open += 1;
                ledger.peak_open = ledger.peak_open.max(ledger.open);
                ledger.open_bytes += canonical.len();
                ledger.peak_open_bytes = ledger.peak_open_bytes.max(ledger.open_bytes);
            }
            let delay = self.delay_ms.get(position).copied().unwrap_or(0);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            {
                let mut ledger = self.ledger();
                ledger.open -= 1;
                ledger.open_bytes -= canonical.len();
                ledger.completed.push(position);
            }
            if self.fails_at.contains(&position) {
                return Err(AuthorityError::Provider {
                    operation: "PutObject",
                    reason: format!("body {position}"),
                });
            }
            Ok(self.outcome)
        }
    }

    #[test]
    fn the_in_flight_byte_budget_is_the_registered_decoded_request_ceiling() {
        const {
            assert!(
                STAGING_BYTE_BUDGET <= OtlpLimits::REGISTERED.decoded_max,
                "staging may never hold more bytes in flight than one request may decode into"
            );
        }
        const { assert!(MAX_CONCURRENT_BODY_PUTS > 0) };
        const { assert!(STAGING_BYTE_QUANTUM > 0) };
        // One maximal observation must fit the budget, or a legal batch would
        // wait on permits that can never be issued.
        const {
            assert!(limits::OBSERVATION_NORMALIZED_MAX <= STAGING_BYTE_BUDGET);
        }
    }

    #[tokio::test]
    async fn at_most_the_configured_number_of_body_writes_are_ever_open_at_once() {
        let observations: Vec<PreparedObservation> = (0..64).map(object_body).collect();
        let sink = RecordingSink::new().with_delays(vec![5; 64]);
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_BUDGET);

        let placements = stager.stage_all(&observations).await.expect("every body");

        assert_eq!(placements.len(), 64);
        let ledger = sink.ledger();
        assert_eq!(ledger.launched.len(), 64, "every body reached the sink");
        assert!(
            ledger.peak_open <= MAX_CONCURRENT_BODY_PUTS,
            "{} writes were open at once, above the bound of {MAX_CONCURRENT_BODY_PUTS}",
            ledger.peak_open
        );
        assert!(
            ledger.peak_open > 1,
            "the writes must actually overlap, or nothing was fixed"
        );
    }

    #[tokio::test]
    async fn open_canonical_bytes_never_exceed_the_configured_byte_budget() {
        // A budget deliberately narrower than the write-count bound, so the byte
        // gate is the binding one and its effect is observable.
        let body_bytes = limits::OBS_INLINE_MAX + 1;
        let per_body_permits = body_bytes.div_ceil(STAGING_BYTE_QUANTUM);
        let budget = 3 * per_body_permits * STAGING_BYTE_QUANTUM;
        let observations: Vec<PreparedObservation> = (0..24).map(object_body).collect();
        let sink = RecordingSink::new().with_delays(vec![5; 24]);
        let stager = BodyStager::new(&sink, scope(), budget);

        stager.stage_all(&observations).await.expect("every body");

        let ledger = sink.ledger();
        assert!(
            ledger.peak_open_bytes <= stager.byte_budget(),
            "{} bytes were open against a {}-byte budget",
            ledger.peak_open_bytes,
            stager.byte_budget()
        );
        assert!(
            ledger.peak_open <= 3,
            "the byte budget admits three of these bodies, not {}",
            ledger.peak_open
        );
        assert!(ledger.peak_open > 1, "the byte gate must not serialize");
    }

    #[tokio::test]
    async fn placements_stay_in_observation_order_despite_out_of_order_completion() {
        let observations: Vec<PreparedObservation> = (0..8).map(object_body).collect();
        // The head is slowest, so completion order is the reverse of input order.
        let sink = RecordingSink::new().with_delays((0..8_u64).map(|n| 80 - n * 10).collect());
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_BUDGET);

        let placements = stager.stage_all(&observations).await.expect("every body");

        let ledger = sink.ledger();
        assert_ne!(
            ledger.completed, ledger.launched,
            "the fixture must actually complete out of order"
        );
        for (position, observation) in observations.iter().enumerate() {
            let digest = aex_observation_domain::canonical::sha256_hex(&observation.canonical);
            assert_eq!(
                placements[position],
                BodyPlacement::Object {
                    key: body_object_key(scope(), &digest),
                    digest,
                },
                "position {position} holds another observation's placement"
            );
        }
    }

    #[tokio::test]
    async fn an_inline_body_takes_no_write_slot_and_no_byte_permit() {
        let observations: Vec<PreparedObservation> = (0..32)
            .map(|_| inline_body(limits::OBS_INLINE_MAX))
            .collect();
        let sink = RecordingSink::new();
        // A budget too small to admit a single object body. Inline bodies are
        // unaffected by it, which is the whole claim.
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_QUANTUM);

        let placements = stager.stage_all(&observations).await.expect("every body");

        assert!(placements.iter().all(|p| *p == BodyPlacement::Inline));
        assert_eq!(
            sink.ledger().launched.len(),
            0,
            "an inline body must never reach the object sink"
        );
    }

    #[tokio::test]
    async fn the_earliest_failure_by_position_wins_and_nothing_is_placed() {
        let observations: Vec<PreparedObservation> = (0..64).map(object_body).collect();
        // Position 9 answers long before position 2, and both fail. The tail
        // must not be able to choose the error the caller sees.
        let mut delays = vec![400_u64; 64];
        delays[0] = 1;
        delays[1] = 1;
        delays[2] = 40;
        delays[9] = 1;
        let sink = RecordingSink::new().with_delays(delays).failing_at(&[2, 9]);
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_BUDGET);

        let error = stager
            .stage_all(&observations)
            .await
            .expect_err("a failed body refuses the whole batch");

        match error {
            AuthorityError::Provider { reason, .. } => assert_eq!(reason, "body 2"),
            other => panic!("expected the earliest provider failure, got {other:?}"),
        }
        let ledger = sink.ledger();
        assert!(
            ledger.launched.len() < observations.len(),
            "the batch must stop launching writes after the first failure, launched {}",
            ledger.launched.len()
        );
        assert!(ledger.peak_open <= MAX_CONCURRENT_BODY_PUTS);
    }

    #[tokio::test]
    async fn a_precondition_failure_places_exactly_where_a_fresh_write_placed() {
        let observations: Vec<PreparedObservation> = (0..12).map(object_body).collect();
        let fresh = RecordingSink::new();
        let first = BodyStager::new(&fresh, scope(), STAGING_BYTE_BUDGET)
            .stage_all(&observations)
            .await
            .expect("the first attempt stages every body");

        // The replay path runs the identical helper against objects that are
        // already present, which is what a `412` reports.
        let replayed_sink = RecordingSink::new().reporting(PutOutcome::AlreadyPresent);
        let replayed = BodyStager::new(&replayed_sink, scope(), STAGING_BYTE_BUDGET)
            .stage_all(&observations)
            .await
            .expect("an already-present body is idempotent success");

        assert_eq!(
            first, replayed,
            "equal-intent replay must place identically"
        );
        assert_eq!(replayed_sink.ledger().launched.len(), 12);
    }

    #[tokio::test]
    async fn a_body_larger_than_the_whole_budget_is_refused_rather_than_waiting() {
        let observations = vec![object_body(0)];
        let sink = RecordingSink::new();
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_QUANTUM);

        let error = stager
            .stage_all(&observations)
            .await
            .expect_err("a body that cannot fit the budget can never be admitted");

        assert!(
            matches!(
                error,
                AuthorityError::Store(
                    aex_observation_store_dynamodb::store::StoreError::ItemTooLarge { .. }
                )
            ),
            "{error:?}"
        );
        assert_eq!(sink.ledger().launched.len(), 0);
    }

    #[tokio::test]
    async fn a_maximal_record_descriptor_stays_inside_both_staging_bounds() {
        // The largest legal descriptor: the registered record ceiling, with a
        // canonical footprint at the registered decoded ceiling, mixing bodies
        // that stage in the sink with bodies that stay on the item.
        let mut observations = Vec::with_capacity(limits::OTLP_MAX_RECORDS);
        for position in 0..limits::OTLP_MAX_RECORDS {
            if position % 4 == 0 {
                observations.push(object_body(position));
            } else {
                observations.push(inline_body(100));
            }
        }
        let footprint: usize = observations
            .iter()
            .map(|observation| observation.canonical.len())
            .sum();
        assert_eq!(observations.len(), limits::OTLP_MAX_RECORDS);
        assert!(
            footprint <= OtlpLimits::REGISTERED.decoded_max,
            "the fixture must be a legal descriptor, not an impossible one"
        );

        let sink = RecordingSink::new();
        let stager = BodyStager::new(&sink, scope(), STAGING_BYTE_BUDGET);
        let placements = stager.stage_all(&observations).await.expect("every body");

        assert_eq!(placements.len(), limits::OTLP_MAX_RECORDS);
        let ledger = sink.ledger();
        assert_eq!(ledger.launched.len(), limits::OTLP_MAX_RECORDS / 4);
        assert!(ledger.peak_open <= MAX_CONCURRENT_BODY_PUTS);
        assert!(ledger.peak_open_bytes <= stager.byte_budget());
        for (position, placement) in placements.iter().enumerate() {
            assert_eq!(
                *placement == BodyPlacement::Inline,
                position % 4 != 0,
                "position {position} was placed against its own body"
            );
        }
    }

    #[test]
    fn the_object_prefix_is_the_observation_one_not_the_content_one() {
        let key = body_object_key(scope(), &"ab".repeat(32));
        assert!(key.starts_with(&format!("{BODY_PREFIX}/")), "{key}");
        assert!(key.contains("/ab/ab/"), "{key}");
        assert_ne!(BODY_PREFIX, "content");
    }

    #[test]
    fn equal_bodies_in_two_sessions_never_share_a_deletion_key() {
        let digest = "ab".repeat(32);
        let first = body_object_key(session_scope(3), &digest);
        let second = body_object_key(session_scope(4), &digest);
        let workspace_owned = body_object_key(scope(), &digest);

        assert_ne!(first, second);
        assert_ne!(first, workspace_owned);
        assert!(
            first.starts_with(&format!("{BODY_PREFIX}/{}/sessions/", workspace())),
            "{first}"
        );
        assert!(workspace_owned.contains("/workspace/"), "{workspace_owned}");
    }

    #[tokio::test]
    async fn the_ordered_helper_never_exceeds_its_bound_and_keeps_input_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let open = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let operations = (0..40_usize).map(|position| {
            let open = &open;
            let peak = &peak;
            // The head is slowest, so completion order is the reverse of input.
            let delay = u64::try_from(40 - position).expect("a bounded fixture delay");
            async move {
                let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                open.fetch_sub(1, Ordering::SeqCst);
                Ok(position)
            }
        });

        let settled = settle_bounded_ordered(operations, 4)
            .await
            .expect("every operation settles");

        assert_eq!(settled, (0..40).collect::<Vec<_>>());
        assert!(
            peak.load(Ordering::SeqCst) <= 4,
            "{} operations ran at once against a bound of 4",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn the_ordered_helper_stops_launching_after_the_earliest_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let launched = AtomicUsize::new(0);
        let operations = (0..50_usize).map(|position| {
            let launched = &launched;
            async move {
                launched.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(if position < 3 { 1 } else { 400 })).await;
                if position == 1 {
                    return Err(AuthorityError::Provider {
                        operation: "GetItem",
                        reason: format!("operation {position}"),
                    });
                }
                Ok(position)
            }
        });

        let error = settle_bounded_ordered(operations, 4)
            .await
            .expect_err("a failed operation refuses the whole set");

        match error {
            AuthorityError::Provider { reason, .. } => assert_eq!(reason, "operation 1"),
            other => panic!("expected the earliest failure, got {other:?}"),
        }
        assert!(
            launched.load(Ordering::SeqCst) < 50,
            "nothing new may be launched once the batch has failed"
        );
    }
}
