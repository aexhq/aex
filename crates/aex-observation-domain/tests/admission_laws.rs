//! G1(6) the receipt state machine, G1(7) series claim races and the workspace
//! cardinality ceiling, G1(8) gap laws, plus the frontier and deletion fence.

use aex_observation_domain::batch::{ReceiptState, ReceiptTransition, TransitionError};
use aex_observation_domain::frontier::{
    AcceptedRange, DeletionState, DeletionTransitionError, Frontier, FrontierError, ScopeDeletion,
};
use aex_observation_domain::gap::{GapLedger, GapRevision, GapState, OrdinalRange, TimeWindow};
use aex_observation_domain::series::{SeriesClaims, SeriesError, SeriesHash};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_wire::generated::models::TelemetryGapReason;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::PrefixedId;
use aex_wire::types::Timestamp;
use proptest::prelude::*;

fn digest(byte: u8) -> IntentDigest {
    IntentDigest::from_bytes([byte; 32])
}

fn instant(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("fixture instant is representable")
}

fn gap_id(suffix: &str) -> aex_wire::generated::ids::TelemetryGapId {
    PrefixedId::parse(&format!("gap_{suffix}")).expect("fixture gap id parses")
}

// --- G1(6) receipt state machine -------------------------------------------

#[test]
fn a_fresh_receipt_prepares_and_commits() {
    let prepared = ReceiptState::prepare(digest(1));
    assert_eq!(prepared, ReceiptState::Preparing { intent: digest(1) });
    let committed = prepared
        .apply(ReceiptTransition::Commit)
        .expect("preparing commits");
    assert_eq!(committed, ReceiptState::Committed { intent: digest(1) });
}

#[test]
fn resuming_a_preparation_needs_the_same_intent() {
    let prepared = ReceiptState::prepare(digest(1));
    prepared
        .resume(digest(1))
        .expect("an equal intent resumes the same preparation");
    assert_eq!(
        prepared.resume(digest(2)),
        Err(TransitionError::IntentConflict)
    );
}

#[test]
fn every_transition_out_of_a_terminal_state_is_rejected() {
    let committed = ReceiptState::Committed { intent: digest(1) };
    let aborted = ReceiptState::Aborted { intent: digest(1) };
    for transition in ReceiptTransition::ALL {
        assert_eq!(
            committed.apply(*transition),
            Err(TransitionError::Terminal {
                state: "committed"
            }),
            "committed + {transition:?}"
        );
        assert_eq!(
            aborted.apply(*transition),
            Err(TransitionError::Terminal { state: "aborted" }),
            "aborted + {transition:?}"
        );
    }
}

#[test]
fn a_committed_receipt_never_resumes_a_preparation() {
    let committed = ReceiptState::Committed { intent: digest(1) };
    assert_eq!(
        committed.resume(digest(1)),
        Err(TransitionError::Terminal {
            state: "committed"
        })
    );
}

#[test]
fn abort_is_reachable_only_from_preparing() {
    let aborted = ReceiptState::prepare(digest(3))
        .apply(ReceiptTransition::Abort)
        .expect("preparing aborts");
    assert_eq!(aborted, ReceiptState::Aborted { intent: digest(3) });
}

// --- G1(7) series claims ----------------------------------------------------

fn series(byte: u8) -> SeriesHash {
    SeriesHash::from_bytes([byte; 32])
}

#[test]
fn overlapping_concurrent_claims_count_each_series_once() {
    let mut claims = SeriesClaims::new(10);
    // Two batches introduce overlapping new series; the shared one is claimed once.
    let first = claims
        .claim_all(&[series(1), series(2), series(3)])
        .expect("first batch claims");
    assert_eq!(first.newly_claimed, 3);
    let second = claims
        .claim_all(&[series(3), series(4)])
        .expect("second batch claims");
    assert_eq!(second.newly_claimed, 1);
    assert_eq!(claims.claimed(), 4);
}

#[test]
fn the_cardinality_ceiling_is_never_exceeded_and_the_batch_fails_whole() {
    let mut claims = SeriesClaims::new(3);
    claims.claim_all(&[series(1), series(2)]).expect("fits");
    let error = claims
        .claim_all(&[series(3), series(4)])
        .expect_err("a batch that would cross the ceiling is refused whole");
    assert_eq!(
        error,
        SeriesError::CardinalityExceeded {
            ceiling: 3,
            claimed: 2,
            attempted: 2,
        }
    );
    // Nothing partial landed.
    assert_eq!(claims.claimed(), 2);
    assert!(!claims.is_claimed(series(3)));
}

#[test]
fn releasing_a_provisional_claim_restores_the_exact_count() {
    let mut claims = SeriesClaims::new(4);
    let outcome = claims.claim_all(&[series(1), series(2)]).expect("claims");
    claims.release(&outcome);
    assert_eq!(claims.claimed(), 0);
    // Releasing twice is a no-op, not a negative count.
    claims.release(&outcome);
    assert_eq!(claims.claimed(), 0);
}

#[test]
fn a_series_hash_is_the_pinned_input_set() {
    let a = SeriesHash::compute(
        "ws_01j0000000000000000000000a",
        "http.server.duration",
        "histogram",
        "ms",
        "delta",
        false,
        &[("route".to_owned(), "/v1/runs".to_owned())],
    );
    let b = SeriesHash::compute(
        "ws_01j0000000000000000000000a",
        "http.server.duration",
        "histogram",
        "ms",
        "delta",
        false,
        &[("route".to_owned(), "/v1/runs".to_owned())],
    );
    assert_eq!(a, b, "the same input set hashes identically");
    let different_workspace = SeriesHash::compute(
        "ws_01j0000000000000000000000b",
        "http.server.duration",
        "histogram",
        "ms",
        "delta",
        false,
        &[("route".to_owned(), "/v1/runs".to_owned())],
    );
    assert_ne!(a, different_workspace, "the workspace is part of identity");
    let monotonic = SeriesHash::compute(
        "ws_01j0000000000000000000000a",
        "http.server.duration",
        "histogram",
        "ms",
        "delta",
        true,
        &[("route".to_owned(), "/v1/runs".to_owned())],
    );
    assert_ne!(a, monotonic, "monotonicity is part of identity");
}

// --- G1(8) gap laws ---------------------------------------------------------

fn window(from: i64, to: i64) -> TimeWindow {
    TimeWindow::new(instant(from), instant(to)).expect("a fixture window is ordered")
}

#[test]
fn gap_revisions_are_append_only_and_monotone() {
    let mut ledger = GapLedger::default();
    let opened = GapRevision::open(
        gap_id("01j0000000000000000000000a"),
        SignalSet::from_signal(Signal::Logs),
        TelemetryGapReason::ProducerDropped,
        Some(window(10, 20)),
        instant(10),
    );
    ledger.append(opened.clone()).expect("first revision appends");
    let repaired = opened.repaired("staged_pages", instant(30));
    assert_eq!(repaired.revision, 1);
    ledger.append(repaired.clone()).expect("a later revision appends");
    assert_eq!(
        ledger.append(opened.clone()),
        Err(aex_observation_domain::gap::GapError::RevisionNotMonotone {
            latest: 1,
            attempted: 0,
        })
    );
    assert_eq!(ledger.latest(&opened.gap_id).map(|r| r.state), Some(GapState::Repaired));
    assert_eq!(ledger.revisions(&opened.gap_id).len(), 2);
}

#[test]
fn completeness_considers_only_intersecting_open_gaps() {
    let mut ledger = GapLedger::default();
    let inside = GapRevision::open(
        gap_id("01j0000000000000000000000a"),
        SignalSet::from_signal(Signal::Logs),
        TelemetryGapReason::SpoolLost,
        Some(window(100, 200)),
        instant(100),
    );
    let outside = GapRevision::open(
        gap_id("01j0000000000000000000000b"),
        SignalSet::from_signal(Signal::Logs),
        TelemetryGapReason::SpoolLost,
        Some(window(900, 1_000)),
        instant(900),
    );
    let other_signal = GapRevision::open(
        gap_id("01j0000000000000000000000c"),
        SignalSet::from_signal(Signal::Metrics),
        TelemetryGapReason::SpoolLost,
        Some(window(100, 200)),
        instant(100),
    );
    ledger.append(inside.clone()).expect("appends");
    ledger.append(outside).expect("appends");
    ledger.append(other_signal).expect("appends");

    let query_window = window(150, 300);
    let signals = SignalSet::from_signal(Signal::Logs);
    let intersecting: Vec<_> = ledger.intersecting_open(signals, query_window).collect();
    assert_eq!(intersecting.len(), 1);
    assert_eq!(intersecting[0].gap_id, inside.gap_id);
    assert!(!ledger.is_complete(signals, query_window));

    // Repairing it makes the window complete again.
    ledger
        .append(inside.repaired("index_replay", instant(400)))
        .expect("appends");
    assert!(ledger.is_complete(signals, query_window));
}

#[test]
fn an_unbounded_gap_never_produces_a_missing_interval() {
    let mut ledger = GapLedger::default();
    let unbounded = GapRevision::open(
        gap_id("01j0000000000000000000000d"),
        SignalSet::from_signal(Signal::Spans),
        TelemetryGapReason::AuthorityUnavailable,
        None,
        instant(10),
    );
    assert!(unbounded.unbounded);
    ledger.append(unbounded).expect("appends");
    let signals = SignalSet::from_signal(Signal::Spans);
    let query = window(0, 1_000);
    assert!(ledger.missing_intervals(signals, query).is_empty());
    assert_eq!(ledger.unbounded_open(signals, query).count(), 1);
    assert!(!ledger.is_complete(signals, query), "an unbounded gap is still incomplete");
}

#[test]
fn replay_expired_is_never_produced_by_any_domain_constructor() {
    // O-19: the wire keeps `replay_expired` for vocabulary stability, but with
    // Kinesis removed nothing can expire a replay. Every reason this crate can
    // mint is enumerated here and none of them is `ReplayExpired`.
    for reason in aex_observation_domain::gap::PRODUCIBLE_REASONS {
        assert_ne!(*reason, TelemetryGapReason::ReplayExpired);
    }
    assert!(
        TelemetryGapReason::ALL.contains(&TelemetryGapReason::ReplayExpired),
        "the wire vocabulary keeps the value"
    );
    assert!(
        !aex_observation_domain::gap::PRODUCIBLE_REASONS
            .contains(&TelemetryGapReason::ReplayExpired)
    );
    // And the constructor refuses it outright rather than trusting a caller.
    let refused = GapRevision::try_open(
        gap_id("01j0000000000000000000000e"),
        SignalSet::from_signal(Signal::Logs),
        TelemetryGapReason::ReplayExpired,
        None,
        instant(1),
    );
    assert!(refused.is_err());
}

#[test]
fn an_ordinal_range_is_inclusive_and_ordered() {
    assert!(OrdinalRange::new(5, 4).is_none());
    let range = OrdinalRange::new(5, 9).expect("ordered range");
    assert!(range.contains(5) && range.contains(9) && !range.contains(10));
    assert_eq!(range.len(), 5);
}

// --- frontier and deletion fence -------------------------------------------

#[test]
fn a_frontier_advances_only_over_a_contiguous_range() {
    let mut frontier = Frontier::empty(instant(0));
    let range = AcceptedRange::new(0, 9).expect("ordered range");
    frontier
        .advance(range, 10, 1_024, instant(10))
        .expect("the first contiguous range advances");
    assert_eq!(frontier.next_accepted_seq(), 10);
    assert_eq!(frontier.count(), 10);
    assert_eq!(frontier.logical_bytes(), 1_024);
    assert_eq!(frontier.revision(), 1);

    let skipped = AcceptedRange::new(11, 12).expect("ordered range");
    assert_eq!(
        frontier.advance(skipped, 2, 8, instant(11)),
        Err(FrontierError::NotContiguous {
            expected: 10,
            found: 11
        })
    );
    assert_eq!(frontier.next_accepted_seq(), 10, "a refused advance changes nothing");
}

#[test]
fn a_frontier_records_the_earliest_retained_position() {
    let mut frontier = Frontier::empty(instant(0));
    frontier
        .advance(AcceptedRange::new(0, 0).expect("range"), 1, 2, instant(50))
        .expect("advances");
    assert_eq!(frontier.earliest_accepted_at(), instant(50));
    frontier
        .advance(AcceptedRange::new(1, 1).expect("range"), 1, 2, instant(90))
        .expect("advances");
    assert_eq!(
        frontier.earliest_accepted_at(),
        instant(50),
        "the earliest retained position does not move forward on append"
    );
}

#[test]
fn deletion_fences_before_it_deletes_and_epochs_are_monotone() {
    let mut deletion = ScopeDeletion::none();
    assert_eq!(deletion.state(), DeletionState::None);
    assert_eq!(deletion.epoch(), 0);

    deletion.begin(instant(1)).expect("a fresh scope can be fenced");
    assert_eq!(deletion.state(), DeletionState::Fencing);
    assert_eq!(deletion.epoch(), 1, "fencing advances the epoch");

    assert_eq!(
        deletion.complete(instant(2)),
        Err(DeletionTransitionError::OutOfOrder {
            from: DeletionState::Fencing,
            to: DeletionState::Complete
        }),
        "completion may never skip the delete and verify steps"
    );

    deletion.to_deleting().expect("fencing moves to deleting");
    deletion.to_verifying().expect("deleting moves to verifying");
    deletion.complete(instant(9)).expect("verification completes");
    assert_eq!(deletion.state(), DeletionState::Complete);
    assert_eq!(deletion.epoch(), 1);

    deletion.begin(instant(10)).expect("a second deletion re-fences");
    assert_eq!(deletion.epoch(), 2);
}

#[test]
fn a_pinned_epoch_is_stale_once_deletion_advances() {
    let mut deletion = ScopeDeletion::none();
    let pinned = deletion.epoch();
    assert!(deletion.epoch_is_current(pinned));
    deletion.begin(instant(1)).expect("fences");
    assert!(!deletion.epoch_is_current(pinned));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Concurrent interleavings of claim/release keep the counter exact and the
    /// ceiling inviolate. This is the sequential model the store's conditional
    /// `ADD` on the 256 counter shards must reproduce.
    #[test]
    fn series_claims_stay_exact_under_arbitrary_interleaving(
        script in proptest::collection::vec(
            (proptest::collection::vec(0_u8..16, 1..5), any::<bool>()),
            1..24,
        )
    ) {
        let ceiling = 8_u64;
        let mut claims = SeriesClaims::new(ceiling);
        let mut expected: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
        for (batch, keep) in script {
            let hashes: Vec<SeriesHash> = batch.iter().map(|byte| series(*byte)).collect();
            let novel: std::collections::BTreeSet<u8> = batch
                .iter()
                .copied()
                .filter(|byte| !expected.contains(byte))
                .collect();
            let would_be = expected.len() as u64 + novel.len() as u64;
            match claims.claim_all(&hashes) {
                Ok(outcome) => {
                    prop_assert!(would_be <= ceiling);
                    prop_assert_eq!(outcome.newly_claimed as usize, novel.len());
                    if keep {
                        expected.extend(novel);
                    } else {
                        claims.release(&outcome);
                    }
                }
                Err(SeriesError::CardinalityExceeded { .. }) => {
                    prop_assert!(would_be > ceiling);
                }
            }
            prop_assert_eq!(claims.claimed(), expected.len() as u64);
            prop_assert!(claims.claimed() <= ceiling);
        }
    }

    /// A gap ledger never loses a revision and always reports the highest one.
    #[test]
    fn the_gap_ledger_keeps_every_revision(steps in 1_usize..12) {
        let mut ledger = GapLedger::default();
        let mut revision = GapRevision::open(
            gap_id("01j0000000000000000000000a"),
            SignalSet::from_signal(Signal::Logs),
            TelemetryGapReason::SpoolLost,
            Some(window(0, 10)),
            instant(0),
        );
        ledger.append(revision.clone()).expect("first appends");
        for step in 1..steps {
            revision = revision.revised(instant(step as i64));
            ledger.append(revision.clone()).expect("appends");
        }
        prop_assert_eq!(ledger.revisions(&revision.gap_id).len(), steps);
        prop_assert_eq!(
            ledger.latest(&revision.gap_id).map(|r| r.revision),
            Some(steps as u64 - 1)
        );
    }
}
