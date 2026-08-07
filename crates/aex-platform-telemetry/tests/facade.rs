//! Evidence for the four properties that make this crate safe to depend on:
//! an absent exporter is a no-op, a failing exporter is a counted no-op that
//! does not block, redaction removes every non-public attribute, and a flush
//! honours its deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aex_platform_telemetry::{
    AttributeValue, FailingExporter, FlushOutcome, Handle, InMemoryExporter, Record, Settings,
};
use aex_telemetry_schema::generated::{
    AEX_CUSTOMER_EMAIL, AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, AEX_SESSION_ID,
    EVENT_AEX_PROCESS_STARTED,
};

fn started(plane: &str) -> Record {
    Record::event(EVENT_AEX_PROCESS_STARTED).with(AEX_PLANE, plane.to_owned())
}

#[test]
fn an_absent_exporter_is_a_no_op() {
    let handle = Handle::install(&Settings::lambda(), None);
    assert!(!handle.has_exporter());
    for _ in 0..1_000 {
        handle.emit(started("dev"));
    }
    assert_eq!(
        handle.pending(),
        0,
        "nothing is queued when no exporter is installed"
    );
    assert_eq!(handle.exported(), 0);
    assert_eq!(handle.dropped(), 1_000);
    assert_eq!(handle.accepted(), 0, "nothing is accepted into the queue");
    assert_eq!(
        handle.flush(Duration::from_secs(1)),
        FlushOutcome::NoExporter
    );
    assert_eq!(handle.stats().emitted(), 1_000);
}

#[test]
fn a_snapshot_reports_the_queue_and_every_counter() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(
        &Settings::lambda().with_queue_capacity(2),
        Some(exporter.clone()),
    );
    for _ in 0..5 {
        handle.emit(
            Record::event(EVENT_AEX_PROCESS_STARTED)
                .with(AEX_PLANE, "dev")
                .with(AEX_CUSTOMER_EMAIL, "someone@example.test"),
        );
    }
    let queued = handle.stats();
    assert_eq!(queued.pending, 2);
    assert_eq!(queued.queue_capacity, 2);
    assert_eq!(queued.accepted, 2);
    assert_eq!(queued.dropped, 3);
    assert_eq!(queued.emitted(), 5);
    assert_eq!(queued.exported, 0);
    assert_eq!(queued.redacted_attributes, 5);
    assert_eq!(queued.flush_deadline_exceeded, 0);

    assert_eq!(
        handle.flush(Duration::ZERO),
        FlushOutcome::DeadlineExceeded { pending: 2 }
    );
    let stalled = handle.stats();
    assert_eq!(stalled.flush_deadline_exceeded, 1);

    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 2 }
    );
    let drained = handle.stats();
    assert_eq!(drained.pending, 0);
    assert_eq!(drained.exported, 2);
}

#[test]
fn every_counter_is_monotonic_across_snapshots() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(
        &Settings::lambda().with_queue_capacity(4).with_batch_size(2),
        Some(exporter.clone()),
    );
    let mut previous = handle.stats();
    for round in 0..50 {
        for _ in 0..3 {
            handle.emit(
                Record::event(EVENT_AEX_PROCESS_STARTED)
                    .with(AEX_PLANE, "dev")
                    .with(AEX_SESSION_ID, "018f6a0e-0000-7000-8000-000000000000"),
            );
        }
        let deadline = if round % 2 == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(5)
        };
        let _ = handle.flush(deadline);
        let current = handle.stats();
        assert!(current.accepted >= previous.accepted, "round {round}");
        assert!(current.exported >= previous.exported, "round {round}");
        assert!(current.dropped >= previous.dropped, "round {round}");
        assert!(
            current.redacted_attributes >= previous.redacted_attributes,
            "round {round}"
        );
        assert!(
            current.flush_deadline_exceeded >= previous.flush_deadline_exceeded,
            "round {round}"
        );
        assert_eq!(current.queue_capacity, 4);
        previous = current;
    }
    assert!(previous.exported > 0, "the loop exported something");
    assert!(previous.dropped > 0, "the loop dropped something");
}

#[test]
fn a_failing_exporter_increments_drops_and_does_not_block() {
    let exporter = Arc::new(FailingExporter::new());
    let handle = Handle::install(
        &Settings::lambda()
            .with_queue_capacity(64)
            .with_batch_size(16),
        Some(exporter.clone()),
    );
    for _ in 0..64 {
        handle.emit(started("dev"));
    }
    let started_at = Instant::now();
    let outcome = handle.flush(Duration::from_secs(5));
    assert!(
        started_at.elapsed() < Duration::from_secs(5),
        "a failing export must not block"
    );
    assert_eq!(outcome, FlushOutcome::Drained { exported: 0 });
    assert_eq!(handle.exported(), 0);
    assert_eq!(
        handle.dropped(),
        64,
        "every undeliverable record is counted once"
    );
    assert_eq!(
        handle.pending(),
        0,
        "a failed batch is discarded, not retried"
    );
    assert_eq!(exporter.calls(), 4, "64 records at a batch size of 16");
}

#[test]
fn a_full_queue_drops_the_newest_record_without_blocking() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(
        &Settings::lambda().with_queue_capacity(4),
        Some(exporter.clone()),
    );
    let started_at = Instant::now();
    for _ in 0..10_000 {
        handle.emit(started("dev"));
    }
    assert!(
        started_at.elapsed() < Duration::from_secs(5),
        "emit must never block"
    );
    assert_eq!(
        handle.pending(),
        4,
        "the queue never grows past its capacity"
    );
    assert_eq!(handle.dropped(), 9_996);
    assert_eq!(exporter.calls(), 0, "emit never exports inline");
}

#[test]
fn redaction_removes_every_non_public_attribute() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::lambda(), Some(exporter.clone()));
    handle.emit(
        Record::event(EVENT_AEX_PROCESS_STARTED)
            .with(AEX_PLANE, "dev")
            .with(AEX_REGION, "eu-west-1")
            .with(AEX_DEPLOYABLE, "regional-session-api")
            .with(AEX_SESSION_ID, "018f6a0e-0000-7000-8000-000000000000")
            .with(AEX_CUSTOMER_EMAIL, "someone@example.test")
            .with("aex.never.declared", "whatever"),
    );
    assert_eq!(handle.redacted_attributes(), 3);
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 1 }
    );

    let delivered = exporter.delivered();
    assert_eq!(delivered.len(), 1);
    let record = &delivered[0];
    assert_eq!(record.attributes.len(), 3);
    assert_eq!(
        record.attribute(AEX_PLANE),
        Some(&AttributeValue::Text("dev".to_owned()))
    );
    assert_eq!(
        record.attribute(AEX_SESSION_ID),
        None,
        "an internal identifier never leaves"
    );
    assert_eq!(
        record.attribute(AEX_CUSTOMER_EMAIL),
        None,
        "a forbidden value never leaves"
    );
    assert_eq!(
        record.attribute("aex.never.declared"),
        None,
        "an unclassified key fails closed"
    );
}

#[test]
fn flush_honours_its_deadline_and_reports_what_is_left() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(
        &Settings::lambda()
            .with_queue_capacity(32)
            .with_batch_size(8),
        Some(exporter.clone()),
    );
    for _ in 0..32 {
        handle.emit(started("prd"));
    }

    let outcome = handle.flush(Duration::ZERO);
    assert_eq!(outcome, FlushOutcome::DeadlineExceeded { pending: 32 });
    assert_eq!(exporter.calls(), 0, "an expired deadline exports nothing");
    assert_eq!(
        handle.pending(),
        32,
        "records are returned to the queue, not discarded"
    );
    assert_eq!(handle.deadline_exceeded(), 1);

    let outcome = handle.flush(Duration::from_secs(5));
    assert_eq!(outcome, FlushOutcome::Drained { exported: 32 });
    assert_eq!(handle.pending(), 0);
    assert_eq!(handle.exported(), 32);
    assert_eq!(exporter.delivered().len(), 32);
}

#[test]
fn flushing_an_empty_queue_drains_immediately() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::lambda(), Some(exporter.clone()));
    assert_eq!(
        handle.flush(Duration::ZERO),
        FlushOutcome::Drained { exported: 0 }
    );
    assert_eq!(exporter.calls(), 0);
}

#[test]
fn clones_share_one_queue_and_one_counter_set() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::lambda(), Some(exporter.clone()));
    let clone = handle.clone();
    clone.emit(started("dev"));
    assert_eq!(handle.pending(), 1);
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 1 }
    );
    assert_eq!(clone.exported(), 1);
}
