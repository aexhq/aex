//! The one idempotent-replay combinator.
//!
//! The system this replaces repeated the same recovery idiom in nine modules —
//! catch a conditional race, re-read the receipt, compare the canonical intent,
//! replay the winner or raise a conflict — each with a slightly different error
//! set and a slightly different replay check. Nine slightly different
//! idempotency implementations is nine chances to get idempotency subtly wrong,
//! so there is exactly one here and every mutating adapter method is written on
//! top of it.

use std::future::Future;
use std::time::Duration;

use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;
use async_trait::async_trait;

use crate::attr::{CodecError, Item, ItemBuilder, Row, b, s, stamp};
use crate::component::{Component, KeyError};
use crate::error::{Resolution, RetryPolicy, StoreError};
use crate::plan::Participant;

/// The closed idempotency scope vocabulary.
///
/// A scope is never free text. It is what makes one caller's `Idempotency-Key`
/// unable to collide with another route's, and what makes a receipt readable
/// without knowing which command wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyScope<'a> {
    base: &'static str,
    subject: Option<Component<'a>>,
}

impl<'a> IdempotencyScope<'a> {
    /// Every scope base, in registry order. A value outside this list cannot be
    /// constructed.
    pub const BASES: &'static [&'static str] = &[
        "session.create",
        "session.message",
        "provider_credential.register",
        "registry.set",
        "registry.delete",
        "registry.upload",
        "registry.download",
    ];

    /// The bases that take no subject.
    const SUBJECTLESS: &'static [&'static str] = &["session.create"];

    /// Builds a scope.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::UnknownBase`] for a base outside
    /// [`IdempotencyScope::BASES`], [`ScopeError::SubjectRequired`] or
    /// [`ScopeError::SubjectForbidden`] when the arity is wrong, and
    /// [`ScopeError::Subject`] when the subject could not enter a key.
    pub fn new(base: &'static str, subject: Option<&'a str>) -> Result<Self, ScopeError> {
        if !Self::BASES.contains(&base) {
            return Err(ScopeError::UnknownBase { base });
        }
        let subjectless = Self::SUBJECTLESS.contains(&base);
        match (subjectless, subject) {
            (true, Some(_)) => Err(ScopeError::SubjectForbidden { base }),
            (true, None) => Ok(Self {
                base,
                subject: None,
            }),
            (false, None) => Err(ScopeError::SubjectRequired { base }),
            (false, Some(text)) => Ok(Self {
                base,
                subject: Some(Component::parse(text).map_err(ScopeError::Subject)?),
            }),
        }
    }

    /// The scope as it appears inside a key: `base` or `base:subject`.
    #[must_use]
    pub fn render(&self) -> String {
        match self.subject {
            None => self.base.to_owned(),
            Some(subject) => format!("{}:{}", self.base, subject.as_str()),
        }
    }

    /// The base.
    #[must_use]
    pub const fn base(&self) -> &'static str {
        self.base
    }
}

/// Why a scope could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    /// The base is outside the closed vocabulary.
    #[error("`{base}` is not an idempotency scope")]
    UnknownBase {
        /// The rejected base.
        base: &'static str,
    },
    /// The base names a resource and none was given.
    #[error("scope `{base}` requires a subject")]
    SubjectRequired {
        /// The base.
        base: &'static str,
    },
    /// The base names no resource and one was given.
    #[error("scope `{base}` takes no subject")]
    SubjectForbidden {
        /// The base.
        base: &'static str,
    },
    /// The subject could not enter a key.
    #[error("the scope subject is unusable: {0}")]
    Subject(#[from] KeyError),
}

/// The `sha256` of a caller-chosen replay key, lowercase hex.
///
/// Delegates to `aex_wire::idempotency::key_digest`, which the session domain
/// also derives a receipt's `key_sha256` through: the physical row and the
/// planned item must agree about what "the hashed key" is, and one function is
/// the only way to guarantee that.
#[must_use]
pub fn key_digest(key: &IdempotencyKey) -> String {
    aex_wire::idempotency::key_digest(key.as_str())
}

/// A stored response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptBody {
    /// The whole canonical response, small enough to live on the receipt.
    Inline(Vec<u8>),
    /// A content digest to read the response from.
    Digest(String),
}

/// One durable idempotency receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The rendered scope.
    pub scope: String,
    /// The hashed replay key.
    pub key_sha256: String,
    /// What the winning request asked for.
    pub intent: IntentDigest,
    /// Which response shape the body decodes as.
    pub response_kind: String,
    /// The stored response.
    pub response: ReceiptBody,
    /// When the winner committed.
    pub committed_at: Timestamp,
    /// When the receipt stops being readable.
    ///
    /// Checked explicitly by the reader. TTL reclaims the row afterwards but is
    /// never the fence: AWS deletes within 48 hours, not at the instant (D-24).
    pub expires_at: Timestamp,
}

/// The `itemType` of an idempotency receipt.
///
/// Every regional table that holds receipts holds this one shape. The row codec
/// lives here rather than beside one table's own codecs because a second
/// spelling of a receipt would be a second, subtly different idempotency
/// implementation — exactly what this module exists to prevent.
pub const IDEMPOTENCY_RECEIPT: &str = "idempotency_receipt";

/// The composite key of one receipt: `IDEM#{workspace}#{scope}#{digest}` /
/// `RECEIPT`.
///
/// # Errors
///
/// [`KeyError`] when the rendered scope or the key digest could not enter a key.
pub fn receipt_key(
    workspace: WorkspaceId,
    scope: &str,
    key_sha256_hex: &str,
) -> Result<(String, String), KeyError> {
    let scope = Component::parse(scope)?;
    let digest = Component::parse(key_sha256_hex)?;
    Ok((
        format!("IDEM#{workspace}#{scope}#{digest}"),
        "RECEIPT".to_owned(),
    ))
}

/// Encodes one idempotency receipt.
///
/// The row carries both the epoch-seconds TTL attribute and the explicit
/// `expiresAt` the reader checks. TTL reclaims space where a table enables it;
/// it is never the fence (D-24), which is why a table with TTL disabled — such
/// as `regional-secret-custody` — stores exactly the same row.
///
/// # Errors
///
/// [`KeyError`] when the rendered scope or the key digest could not enter a key.
pub fn encode_receipt_row(workspace: WorkspaceId, receipt: &Receipt) -> Result<Item, KeyError> {
    let (pk, sk) = receipt_key(workspace, &receipt.scope, &receipt.key_sha256)?;
    let builder = ItemBuilder::new(IDEMPOTENCY_RECEIPT)
        .set(crate::attr::PK, s(pk))
        .set(crate::attr::SK, s(sk))
        .set("scope", s(receipt.scope.clone()))
        .set("keySha256", s(receipt.key_sha256.clone()))
        .set("intentHash", s(receipt.intent.to_string()))
        .set("responseKind", s(receipt.response_kind.clone()))
        .set("committedAt", stamp(receipt.committed_at))
        .set("expiresAt", stamp(receipt.expires_at))
        .set(
            "expiresAtEpochSeconds",
            crate::attr::n_i64(receipt.expires_at.unix_millis().div_euclid(1_000)),
        );
    Ok(match &receipt.response {
        ReceiptBody::Inline(bytes) => builder.set("responseInline", b(bytes.clone())),
        ReceiptBody::Digest(digest) => builder.set("responseDigest", s(digest.clone())),
    }
    .build())
}

/// Decodes one idempotency receipt.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or malformed attribute.
pub fn decode_receipt_row(item: &Item) -> Result<Receipt, CodecError> {
    let row = Row::bind(item, IDEMPOTENCY_RECEIPT)?;
    let intent = parse_intent(row.string("intentHash")?).ok_or(CodecError::Malformed {
        item_type: IDEMPOTENCY_RECEIPT,
        attribute: "intentHash",
        reason: "an intent hash is 64 lowercase hex characters".to_owned(),
    })?;
    let response = match (
        row.opt_bytes("responseInline")?,
        row.opt_string("responseDigest")?,
    ) {
        (Some(bytes), _) => ReceiptBody::Inline(bytes.to_vec()),
        (None, Some(digest)) => ReceiptBody::Digest(digest.to_owned()),
        (None, None) => {
            return Err(CodecError::Missing {
                item_type: IDEMPOTENCY_RECEIPT,
                attribute: "responseInline",
            });
        }
    };
    Ok(Receipt {
        scope: row.string("scope")?.to_owned(),
        key_sha256: row.string("keySha256")?.to_owned(),
        intent,
        response_kind: row.string("responseKind")?.to_owned(),
        response,
        committed_at: row.timestamp("committedAt")?,
        expires_at: row.timestamp("expiresAt")?,
    })
}

/// Whether a receipt is still readable at `now`.
///
/// This is the fence, not the TTL attribute: AWS deletes a TTL'd row within 48
/// hours, not at the instant, so a reader that trusted TTL would replay an
/// expired receipt for up to two days.
#[must_use]
pub fn receipt_is_live(receipt: &Receipt, now: Timestamp) -> bool {
    now.unix_millis() < receipt.expires_at.unix_millis()
}

/// Parses a 64-character lowercase-hex intent digest.
///
/// One parser, shared by every row that stores an `intentHash`: a second one
/// could accept a spelling the first rejects.
#[must_use]
pub fn parse_intent(text: &str) -> Option<IntentDigest> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(IntentDigest::from_bytes(bytes))
}

/// How long a receipt is retained (OD-16).
#[allow(
    clippy::duration_suboptimal_units,
    reason = "`Duration::from_hours` is not const-stable on the pinned toolchain"
)]
pub const RECEIPT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a durable operation record is retained after a terminal state
/// (OD-16).
#[allow(
    clippy::duration_suboptimal_units,
    reason = "`Duration::from_days` is not const-stable on the pinned toolchain"
)]
pub const OPERATION_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A response that can be reconstructed from a receipt.
pub trait DecodeReceipt: Sized {
    /// Rebuilds the original response.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] when the stored body is not this response.
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError>;
}

/// The receipt read side, so the combinator does not have to know which table
/// holds the receipts.
#[async_trait]
pub trait ReceiptStore: Send + Sync {
    /// Reads one receipt.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`] the underlying read produces. An expired receipt is
    /// reported as absent, never as a stale hit.
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError>;
}

/// Bounded, jittered waiting between retries.
///
/// The adapter reads no clock and no random source itself; the composition
/// supplies one implementation and a test supplies a recording no-op, which is
/// what makes the retry bound assertable without a timer.
#[async_trait]
pub trait Backoff: Send + Sync {
    /// Waits before `attempt`, counting from one.
    async fn wait(&self, policy: RetryPolicy, attempt: u32);
}

/// The outcome of a replay-protected commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replayable<R> {
    /// This call committed the effect.
    Committed(R),
    /// An earlier identical call committed it; this is that response.
    Replayed(R),
}

impl<R> Replayable<R> {
    /// The response, however it was obtained.
    pub fn into_inner(self) -> R {
        match self {
            Self::Committed(value) | Self::Replayed(value) => value,
        }
    }

    /// Whether the effect was already committed before this call.
    #[must_use]
    pub const fn is_replay(&self) -> bool {
        matches!(self, Self::Replayed(_))
    }
}

/// Commits an effect exactly once under a replay identity.
///
/// The contract, in order:
///
/// 1. read the receipt first; a hit with an equal `intent` is a replay, a hit
///    with a different `intent` is [`StoreError::IdempotencyConflict`];
/// 2. otherwise run `commit`, which **must** include the receipt `Put` under
///    `attribute_not_exists(pk)` as a participant of its own transaction;
/// 3. on a precondition failure naming `receipt_participant`, or on
///    [`StoreError::CommitAmbiguous`], re-read the receipt once and apply rule 1;
/// 4. a precondition failure on any *other* participant propagates unchanged —
///    the combinator never converts a domain precondition failure into a
///    conflict;
/// 5. [`StoreError::Contended`] and throttling re-enter at step 1 under the
///    bounded policy, so a retry can never produce a second commit.
///
/// # Errors
///
/// Every [`StoreError`] the receipt read or `commit` produces, plus
/// [`StoreError::IdempotencyConflict`] for a key reused with a different intent.
pub async fn commit_or_replay<R, F, Fut>(
    receipts: &dyn ReceiptStore,
    backoff: &dyn Backoff,
    request: ReplayRequest<'_>,
    commit: F,
) -> Result<Replayable<R>, StoreError>
where
    R: DecodeReceipt,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<R, StoreError>>,
{
    let policy = request.policy;
    let mut attempt = 1;
    loop {
        // Step 1. The receipt is always read first, so a retry that re-enters
        // here observes its own earlier commit rather than making a second one.
        if let Some(receipt) = receipts
            .read_receipt(request.workspace, &request.scope, request.key, request.now)
            .await?
        {
            return decide(&receipt, request.intent).map(Replayable::Replayed);
        }

        // Step 2.
        match commit().await {
            Ok(value) => return Ok(Replayable::Committed(value)),

            // Step 3. The receipt participant lost, or the outcome is unknown:
            // exactly one re-read decides it.
            Err(StoreError::PreconditionFailed { participant, .. })
                if participant == request.receipt_participant =>
            {
                return resolve_by_receipt(receipts, &request).await;
            }
            Err(StoreError::CommitAmbiguous { resolve_by }) => {
                if resolve_by != Resolution::IdempotencyReceipt {
                    return Err(StoreError::CommitAmbiguous { resolve_by });
                }
                return resolve_by_receipt(receipts, &request).await;
            }
            Err(StoreError::IdempotencyConflict) => {
                // The provider rejected the transport token. The durable
                // receipt is still the product authority, so ask it.
                return resolve_by_receipt(receipts, &request).await;
            }

            // Step 5.
            Err(error) if error.retryable() && attempt < policy.attempts => {
                attempt += 1;
                backoff.wait(policy, attempt).await;
            }

            // Step 4, and every terminal failure.
            Err(error) => return Err(error),
        }
    }
}

/// Everything `commit_or_replay` needs that is not the commit itself.
#[derive(Debug, Clone, Copy)]
pub struct ReplayRequest<'a> {
    /// The workspace the receipt is partitioned under.
    pub workspace: WorkspaceId,
    /// The scope.
    pub scope: IdempotencyScope<'a>,
    /// The caller-chosen key.
    pub key: &'a IdempotencyKey,
    /// The canonical intent of this request.
    pub intent: IntentDigest,
    /// Which participant writes the receipt.
    pub receipt_participant: Participant,
    /// The retry policy.
    pub policy: RetryPolicy,
    /// The request clock, used to reject an expired receipt.
    pub now: Timestamp,
}

async fn resolve_by_receipt<R: DecodeReceipt>(
    receipts: &dyn ReceiptStore,
    request: &ReplayRequest<'_>,
) -> Result<Replayable<R>, StoreError> {
    // Exactly one re-read: a loop here would turn an ambiguous commit into an
    // unbounded poll against an authority that may never answer differently.
    match receipts
        .read_receipt(request.workspace, &request.scope, request.key, request.now)
        .await?
    {
        Some(receipt) => decide(&receipt, request.intent).map(Replayable::Replayed),
        // Rule 3's re-read found nothing. The write did not commit and the
        // caller must decide; reporting success here would invent an effect.
        None => Err(StoreError::CommitAmbiguous {
            resolve_by: Resolution::TargetItem,
        }),
    }
}

fn decide<R: DecodeReceipt>(receipt: &Receipt, intent: IntentDigest) -> Result<R, StoreError> {
    if receipt.intent == intent {
        R::decode_receipt(receipt)
    } else {
        Err(StoreError::IdempotencyConflict)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
    use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;
    use async_trait::async_trait;

    use super::{
        Backoff, DecodeReceipt, IdempotencyScope, Receipt, ReceiptBody, ReceiptStore,
        ReplayRequest, ScopeError, commit_or_replay, key_digest,
    };
    use crate::error::{Resolution, RetryPolicy, StoreError};
    use crate::plan::Participant;

    #[derive(Debug, PartialEq, Eq)]
    struct Response(String);

    impl DecodeReceipt for Response {
        fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
            match &receipt.response {
                ReceiptBody::Inline(bytes) => Ok(Self(String::from_utf8_lossy(bytes).into_owned())),
                ReceiptBody::Digest(digest) => Ok(Self(digest.clone())),
            }
        }
    }

    /// A receipt store whose row appears only after `appears_after` reads, so a
    /// test can model "the receipt was written by the racing winner between my
    /// first read and my commit".
    struct Fake {
        receipt: Option<Receipt>,
        appears_after: usize,
        reads: AtomicUsize,
    }

    impl Fake {
        fn always(receipt: Option<Receipt>) -> Self {
            Self {
                receipt,
                appears_after: 0,
                reads: AtomicUsize::new(0),
            }
        }

        fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl ReceiptStore for Fake {
        async fn read_receipt(
            &self,
            _workspace: WorkspaceId,
            _scope: &IdempotencyScope<'_>,
            _key: &IdempotencyKey,
            _now: Timestamp,
        ) -> Result<Option<Receipt>, StoreError> {
            let seen = self.reads.fetch_add(1, Ordering::Relaxed) + 1;
            if seen > self.appears_after {
                Ok(self.receipt.clone())
            } else {
                Ok(None)
            }
        }
    }

    struct CountingBackoff {
        waits: AtomicU32,
    }

    impl CountingBackoff {
        fn new() -> Self {
            Self {
                waits: AtomicU32::new(0),
            }
        }

        fn waits(&self) -> u32 {
            self.waits.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl Backoff for CountingBackoff {
        async fn wait(&self, _policy: RetryPolicy, _attempt: u32) {
            self.waits.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn intent(byte: u8) -> IntentDigest {
        IntentDigest::from_bytes([byte; 32])
    }

    fn receipt(intent_byte: u8) -> Receipt {
        Receipt {
            scope: "session.create".to_owned(),
            key_sha256: "00".repeat(32),
            intent: intent(intent_byte),
            response_kind: "session".to_owned(),
            response: ReceiptBody::Inline(b"stored".to_vec()),
            committed_at: Timestamp::from_unix_millis(0).expect("epoch"),
            expires_at: Timestamp::from_unix_millis(86_400_000).expect("a day later"),
        }
    }

    fn request(key: &IdempotencyKey, intent_byte: u8) -> ReplayRequest<'_> {
        ReplayRequest {
            workspace: workspace(),
            scope: IdempotencyScope::new("session.create", None).expect("a known base"),
            key,
            intent: intent(intent_byte),
            receipt_participant: Participant::SESSION_IDEMPOTENCY,
            policy: RetryPolicy::PINNED,
            now: Timestamp::from_unix_millis(1_000).expect("in range"),
        }
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    #[test]
    fn an_existing_receipt_with_an_equal_intent_replays_without_committing() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake::always(Some(receipt(7)));
        let backoff = CountingBackoff::new();
        let outcome = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 7),
            || async { panic!("commit must not run when a receipt already exists") },
        ))
        .expect("a replay");
        assert!(outcome.is_replay());
        assert_eq!(outcome.into_inner(), Response("stored".to_owned()));
    }

    #[test]
    fn an_existing_receipt_with_a_different_intent_is_a_conflict() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake::always(Some(receipt(7)));
        let backoff = CountingBackoff::new();
        let error = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 9),
            || async { unreachable!() },
        ))
        .expect_err("a conflict");
        assert_eq!(error, StoreError::IdempotencyConflict);
    }

    #[test]
    fn a_precondition_failure_on_another_participant_propagates_unchanged() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake::always(None);
        let backoff = CountingBackoff::new();
        let error = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 1),
            || async {
                Err(StoreError::PreconditionFailed {
                    participant: Participant::SESSION_HEAD,
                    observed: None,
                })
            },
        ))
        .expect_err("the head condition lost");
        assert!(
            matches!(
                error,
                StoreError::PreconditionFailed {
                    participant: Participant::SESSION_HEAD,
                    ..
                }
            ),
            "a domain precondition failure must never become a conflict: {error}"
        );
    }

    #[test]
    fn a_lost_receipt_race_replays_the_winner_after_exactly_one_re_read() {
        let key = IdempotencyKey::parse("k").expect("a key");
        // Absent on the first read, present on the second: the racing winner
        // committed between this caller's read and its own commit.
        let store = Fake {
            receipt: Some(receipt(3)),
            appears_after: 1,
            reads: AtomicUsize::new(0),
        };
        let backoff = CountingBackoff::new();
        let outcome = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 3),
            || async {
                Err(StoreError::PreconditionFailed {
                    participant: Participant::SESSION_IDEMPOTENCY,
                    observed: None,
                })
            },
        ))
        .expect("the winner's response");
        assert!(outcome.is_replay());
        assert_eq!(store.reads(), 2, "one read before, exactly one after");
    }

    #[test]
    fn an_ambiguous_commit_that_did_land_replays_rather_than_committing_twice() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake {
            receipt: Some(receipt(3)),
            appears_after: 1,
            reads: AtomicUsize::new(0),
        };
        let backoff = CountingBackoff::new();
        let outcome = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 3),
            || async {
                Err(StoreError::CommitAmbiguous {
                    resolve_by: Resolution::IdempotencyReceipt,
                })
            },
        ))
        .expect("resolved by the receipt");
        assert!(outcome.is_replay());
        assert_eq!(store.reads(), 2);
    }

    #[test]
    fn an_ambiguous_commit_with_no_receipt_stays_ambiguous_rather_than_inventing_success() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake::always(None);
        let backoff = CountingBackoff::new();
        let error = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 1),
            || async {
                Err(StoreError::CommitAmbiguous {
                    resolve_by: Resolution::IdempotencyReceipt,
                })
            },
        ))
        .expect_err("still unresolved");
        assert_eq!(
            error,
            StoreError::CommitAmbiguous {
                resolve_by: Resolution::TargetItem
            }
        );
        assert_eq!(store.reads(), 2, "one read before, exactly one after");
    }

    #[test]
    fn contention_is_retried_a_bounded_number_of_times_and_re_enters_at_the_receipt_read() {
        let key = IdempotencyKey::parse("k").expect("a key");
        let store = Fake::always(None);
        let backoff = CountingBackoff::new();
        let error = run(commit_or_replay::<Response, _, _>(
            &store,
            &backoff,
            request(&key, 1),
            || async { Err(StoreError::Contended) },
        ))
        .expect_err("contention all the way down");
        assert_eq!(error, StoreError::Contended);
        assert_eq!(store.reads(), RetryPolicy::PINNED.attempts as usize);
        assert_eq!(backoff.waits(), RetryPolicy::PINNED.attempts - 1);
    }

    #[test]
    fn a_scope_outside_the_vocabulary_cannot_be_built() {
        assert_eq!(
            IdempotencyScope::new("session.teleport", None).unwrap_err(),
            ScopeError::UnknownBase {
                base: "session.teleport"
            }
        );
    }

    #[test]
    fn scope_arity_is_enforced_in_both_directions() {
        assert!(matches!(
            IdempotencyScope::new("session.create", Some("ses_x")),
            Err(ScopeError::SubjectForbidden { .. })
        ));
        assert!(matches!(
            IdempotencyScope::new("session.message", None),
            Err(ScopeError::SubjectRequired { .. })
        ));
        assert_eq!(
            IdempotencyScope::new("session.message", Some("ses_x"))
                .expect("a subject")
                .render(),
            "session.message:ses_x"
        );
        assert_eq!(
            IdempotencyScope::new("provider_credential.register", Some("openai"))
                .expect("a provider subject")
                .render(),
            "provider_credential.register:openai"
        );
    }

    #[test]
    fn a_scope_subject_can_never_carry_the_key_separator() {
        assert!(matches!(
            IdempotencyScope::new("session.message", Some("ses#evil")),
            Err(ScopeError::Subject(_))
        ));
    }

    #[test]
    fn the_replay_key_is_hashed_rather_than_placed_in_the_partition_key() {
        let key = IdempotencyKey::parse("a key with spaces and #").expect("a key");
        let digest = key_digest(&key);
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert!(!digest.contains('#'));
    }
}
