//! Contract evidence for the mounted route surface.
//!
//! The mounted set is derived from the generated route table, so a route that is
//! authored and never mounted is a failure here rather than a runtime `404`.

use aex_wire::idempotency::IdempotencyKind;
use aex_wire::routes::{BodyClass, Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;

/// The two groups this deployable owns.
const GROUPS: [RouteGroup; 2] = [RouteGroup::Observations, RouteGroup::TelemetryLifecycle];

fn owned() -> Vec<RouteId> {
    GROUPS
        .iter()
        .flat_map(|group| group.routes().iter().copied())
        .collect()
}

#[test]
fn this_deployable_owns_the_thirty_nine_observation_and_twelve_lifecycle_routes() {
    assert_eq!(RouteGroup::Observations.routes().len(), 39);
    assert_eq!(RouteGroup::TelemetryLifecycle.routes().len(), 12);
    assert_eq!(owned().len(), 51);
}

#[test]
fn the_groups_partition_cleanly_from_every_other_deployable() {
    for id in RouteId::ALL {
        let mine = owned().contains(id);
        assert_eq!(
            mine,
            GROUPS.contains(&id.group()),
            "`{}` disagrees with its own group",
            route(*id).operation_id
        );
    }
}

#[test]
fn every_owned_route_is_regional_and_carries_no_otlp_body() {
    for id in owned() {
        let descriptor = route(id);
        assert_eq!(descriptor.plane, Plane::Regional);
        assert_ne!(
            descriptor.body_class,
            BodyClass::Otlp,
            "`{}` is an admission route and belongs to regional-otlp",
            descriptor.operation_id
        );
    }
}

#[test]
fn the_ndjson_routes_are_exactly_the_stream_and_listen_operations() {
    let streaming: Vec<&'static str> = owned()
        .into_iter()
        .filter(|id| route(*id).transport == TransportKind::Ndjson)
        .map(|id| route(id).operation_id)
        .collect();
    assert!(!streaming.is_empty(), "the group declares NDJSON routes");
    for operation in &streaming {
        assert!(
            operation.ends_with("_stream") || operation.ends_with("_listen"),
            "`{operation}` is not a replay or follow route"
        );
    }
    // Every one of them needs a frame stream, which is why this deployable
    // supplies the `FrameStream` bound the wire deliberately leaves open.
    assert_eq!(streaming.len(), 24);
}

#[test]
fn the_export_admission_routes_require_a_durable_operation_identity() {
    for id in [
        RouteId::TelemetryExportCreate,
        RouteId::SessionTelemetryExportCreate,
    ] {
        let descriptor = route(id);
        assert_eq!(descriptor.success_status, 202);
        assert_eq!(
            descriptor.idempotency,
            IdempotencyKind::OperationId,
            "an export is a durable operation"
        );
    }
}

#[test]
fn no_clickhouse_or_kinesis_dependency_can_reach_this_binary() {
    let manifest = include_str!("../Cargo.toml");
    for banned in ["clickhouse", "kinesis"] {
        assert!(
            !manifest.to_ascii_lowercase().contains(banned),
            "`{banned}` must not appear anywhere in this deployable's dependency set"
        );
    }
}
