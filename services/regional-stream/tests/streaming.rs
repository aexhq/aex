//! Stream origin, wake, quota, route and drain requirements.

use aex_wire::routes::{RouteId, TransportKind, route};
use regional_stream::{
    ConnectionClass, DrainCoordinator, QuotaLimits, QuotaManager, StreamOrigin, WakeMode,
    authoritative_after_wakes, stream_route_ids,
};

#[test]
fn origin_is_exactly_one_for_stream_and_absent_for_listen() {
    assert!(StreamOrigin::for_stream(Some("cur_1"), None, false).is_ok());
    assert!(StreamOrigin::for_stream(None, Some(1_000), false).is_ok());
    assert!(StreamOrigin::for_stream(None, None, true).is_ok());
    assert!(StreamOrigin::for_stream(None, None, false).is_err());
    assert!(StreamOrigin::for_stream(Some("cur_1"), Some(1_000), false).is_err());
    assert!(StreamOrigin::for_listen(None, None, false).is_ok());
    assert!(StreamOrigin::for_listen(Some("cur_1"), None, false).is_err());
}

#[test]
fn wake_modes_are_configured_fail_closed_and_payloads_are_never_emitted() {
    assert!(WakeMode::parse("ddb_streams", 2).is_ok());
    assert!(WakeMode::parse("ddb_streams", 3).is_err());
    assert!(WakeMode::parse("poll", 100).is_ok());
    assert!(WakeMode::parse("unknown", 1).is_err());
    let authority = vec![1, 2, 3, 4, 5];
    let with_loss = authoritative_after_wakes(&authority, 2, &[5]);
    let with_duplicates = authoritative_after_wakes(&authority, 2, &[1, 1, 9, 2]);
    assert_eq!(with_loss, vec![3, 4, 5]);
    assert_eq!(with_duplicates, with_loss);
}

#[test]
fn quotas_isolate_classes_and_charge_telemetry_to_both() {
    let manager = QuotaManager::new(QuotaLimits {
        total: 3,
        session: 1,
        observation: 2,
        per_workspace: 2,
    })
    .expect("limits");
    let session = manager
        .reserve("workspace-a", ConnectionClass::Session)
        .expect("session");
    assert!(
        manager
            .reserve("workspace-b", ConnectionClass::Session)
            .is_err()
    );
    let observation = manager
        .reserve("workspace-b", ConnectionClass::Observation)
        .expect("observation");
    assert!(
        manager
            .reserve("workspace-a", ConnectionClass::Telemetry)
            .is_err()
    );
    drop(session);
    let telemetry = manager
        .reserve("workspace-a", ConnectionClass::Telemetry)
        .expect("telemetry");
    assert_eq!(manager.counts(), (2, 1, 2));
    drop(observation);
    drop(telemetry);
    assert_eq!(manager.counts(), (0, 0, 0));
}

#[test]
fn drain_flips_readiness_and_preserves_each_exact_sent_cursor() {
    let mut drain = DrainCoordinator::new();
    drain.register(1, "cur_one");
    drain.register(2, "cur_two");
    assert!(drain.is_ready());
    let frames = drain.begin();
    assert!(!drain.is_ready());
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].cursor(), "cur_one");
    assert_eq!(frames[1].cursor(), "cur_two");
}

#[test]
fn regional_stream_owns_all_and_only_generated_ndjson_routes() {
    let routes = stream_route_ids();
    assert_eq!(routes.len(), 24);
    assert!(routes.contains(&RouteId::ObservationsEventsStream));
    assert!(routes.contains(&RouteId::SessionObservationsTelemetryListen));
    assert!(!routes.contains(&RouteId::ObservationsEventsQuery));
    assert!(
        routes
            .iter()
            .all(|id| route(*id).transport == TransportKind::Ndjson)
    );
}
