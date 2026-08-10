//! G7 — the `PERF-08` staged-commit proof. **Gating.**
//!
//! A maximal batch is 2,000 points inside a 16 MiB decoded payload. This suite
//! proves that admitting one fits `DynamoDB`'s 100-action / 4 MiB
//! `TransactWriteItems` envelope, under concurrent series claims, at every
//! signal fan-out the protocol permits.
//!
//! If a measurement here fails, the **protocol** changes in
//! `aex-observation-store-dynamodb` — the staged-page size, the action decomposition,
//! or the materialization boundary. Lowering the public 2,000-point limit is the
//! last resort, is recorded, and is never a silent cap.

use aex_observation_domain::keys::{BucketHour, ScopeKey};
use aex_observation_domain::limits;
use aex_observation_domain::series::{SeriesClaims, SeriesHash};
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_observation_store_dynamodb::store::{AdmissionPlan, StagedRecord};
use aex_wire::ids::PrefixedId as _;

/// The maximum public batch.
const MAX_RECORDS: usize = limits::OTLP_MAX_RECORDS;

/// The maximum decoded payload one batch may carry.
const MAX_DECODED_BYTES: usize = limits::OTLP_DECODED_MAX;

fn scope() -> ScopeKey {
    ScopeKey::Session {
        workspace: aex_wire::ids::WorkspaceId::parse("wsp_0000000001e40r2081040g2081")
            .expect("fixture parses"),
        session: aex_wire::ids::SessionId::parse("ses_0000000003ec1r60r30c1g60r3")
            .expect("fixture parses"),
    }
}

fn bucket() -> BucketHour {
    BucketHour::parse("2026-08-01T09").expect("fixture parses")
}

/// A maximal batch: 2,000 records whose canonical bytes sum to the decoded
/// ceiling, so each record is at the largest average size the protocol admits.
fn maximal_records(signals: &[Signal]) -> Vec<StagedRecord> {
    let per_record = MAX_DECODED_BYTES / MAX_RECORDS;
    (0..MAX_RECORDS)
        .map(|index| StagedRecord {
            signal: signals[index % signals.len()],
            canonical: vec![b'x'; per_record],
        })
        .collect()
}

#[test]
fn a_maximal_batch_fits_the_transaction_envelope_at_every_signal_fan_out() {
    // Every fan-out the protocol permits: one signal through all four that live
    // in `observation-authority`. `events` are in `session-authority` and are
    // never part of an admission transaction.
    let fan_outs: [&[Signal]; 4] = [
        &[Signal::Logs],
        &[Signal::Logs, Signal::Spans],
        &[Signal::Logs, Signal::Spans, Signal::Metrics],
        &[Signal::Logs, Signal::Spans, Signal::Metrics, Signal::Traces],
    ];

    for signals in fan_outs {
        let set: SignalSet = signals.iter().copied().collect();
        // The worst case for the commit is a batch that also claims new series.
        let plan = AdmissionPlan::new(scope(), set, bucket(), &maximal_records(signals), 512)
            .expect("a maximal batch stages into page items under the item ceiling");

        let prepare = plan
            .prepare_envelope()
            .require_fits("P")
            .expect("transaction P fits the envelope");
        let commit = plan
            .commit_envelope()
            .require_fits("C")
            .expect("transaction C fits the envelope");

        assert!(
            commit.actions <= limits::COMMIT_MAX_ACTIONS,
            "fan-out {}: transaction C needs {} actions, above the designed \
             {}-action ceiling; change the protocol, never the public limit",
            signals.len(),
            commit.actions,
            limits::COMMIT_MAX_ACTIONS
        );
        assert!(
            commit.bytes <= limits::DDB_TRANSACT_MAX_BYTES,
            "fan-out {}: transaction C serializes to {} bytes, above the 4 MiB envelope",
            signals.len(),
            commit.bytes
        );
        assert!(prepare.actions <= limits::DDB_TRANSACT_MAX_ACTIONS);

        // The measurements, reported rather than merely asserted, so the gate's
        // output names the headroom it proved.
        println!(
            "G7 fan-out={} records={} pages={} P(actions={}, bytes={}) \
             C(actions={}, bytes={}) materialization_batches={}",
            signals.len(),
            plan.records(),
            plan.page_count(),
            prepare.actions,
            prepare.bytes,
            commit.actions,
            commit.bytes,
            plan.materialization_batches()
        );
    }
}

#[test]
fn the_commit_action_count_is_independent_of_the_batch_size() {
    let set = SignalSet::from_signal(Signal::Logs);
    let one = AdmissionPlan::new(
        scope(),
        set,
        bucket(),
        &[StagedRecord {
            signal: Signal::Logs,
            canonical: vec![b'x'; 16],
        }],
        0,
    )
    .expect("plans");
    let maximal = AdmissionPlan::new(scope(), set, bucket(), &maximal_records(&[Signal::Logs]), 0)
        .expect("plans");

    assert_eq!(
        one.commit_envelope().actions,
        maximal.commit_envelope().actions,
        "the commit's action count must not grow with the record count, \
         or the 2,000-point batch would need a second transaction"
    );
    assert!(maximal.commit_envelope().bytes > one.commit_envelope().bytes);
    assert!(maximal.commit_envelope().fits());
}

#[test]
fn ten_concurrent_writers_claiming_overlapping_series_stay_inside_the_ceiling() {
    // The store's conditional `ADD` over the 256 `SERIESCT#` shards must
    // reproduce this sequential model exactly: overlapping claims count once,
    // and a batch that would cross the ceiling is refused whole.
    let ceiling = 250u64;
    let mut claims = SeriesClaims::new(ceiling);
    let mut admitted = 0u64;

    for writer in 0..10u8 {
        // Each writer introduces 30 series, half of which overlap its neighbour.
        let batch: Vec<SeriesHash> = (0..30u8)
            .map(|index| SeriesHash::from_bytes([writer.wrapping_mul(15).wrapping_add(index); 32]))
            .collect();
        match claims.claim_all(&batch) {
            Ok(outcome) => {
                admitted += outcome.newly_claimed;
                let plan = AdmissionPlan::new(
                    scope(),
                    SignalSet::from_signal(Signal::Metrics),
                    bucket(),
                    &maximal_records(&[Signal::Metrics]),
                    usize::try_from(outcome.newly_claimed).expect("fits"),
                )
                .expect("plans");
                plan.commit_envelope()
                    .require_fits("C")
                    .expect("transaction C fits with a concurrent series claim");
            }
            Err(error) => panic!("the fixture must stay inside the ceiling: {error}"),
        }
        assert!(claims.claimed() <= ceiling);
    }
    assert_eq!(claims.claimed(), admitted);
}

#[test]
fn crash_cleanup_is_bounded_at_every_boundary() {
    // The staged pages a crash leaves behind are exactly what the reconciler
    // replays from, so their count is what bounds the cleanup work.
    let plan = AdmissionPlan::new(
        scope(),
        SignalSet::from_signal(Signal::Logs),
        bucket(),
        &maximal_records(&[Signal::Logs]),
        0,
    )
    .expect("plans");

    // At the maximum batch the *byte* budget binds, not the record count: 2,000
    // records averaging 8,388 canonical bytes pack 22 to a 192 KiB page, so the
    // page count is the byte-bound ceiling division rather than the record one.
    let per_record = MAX_DECODED_BYTES / MAX_RECORDS;
    let per_page = limits::OBS_PAGE_MAX_BYTES / per_record;
    assert!(
        per_page < limits::OBS_PAGE_RECORDS,
        "the byte budget binds here"
    );
    assert_eq!(
        plan.page_count(),
        MAX_RECORDS.div_ceil(per_page),
        "the staged-page count is the byte-bound ceiling division"
    );
    assert!(
        plan.page_count() <= limits::DDB_TRANSACT_MAX_ACTIONS,
        "cleanup of one batch's staged pages must fit a bounded number of calls"
    );
    assert_eq!(
        plan.materialization_batches(),
        MAX_RECORDS.div_ceil(25),
        "step 9 is a bounded number of BatchWriteItem calls, not a transaction"
    );
}
