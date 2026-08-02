//! Contract evidence for the generated route ownership surface.
//!
//! Runtime mounting is deliberately narrower while telemetry-export admission
//! lacks one total operation authority; the production-router regression lives
//! beside the mount implementation.

use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::idempotency::IdempotencyKind;
use aex_wire::routes::{BodyClass, Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;

/// The two groups this deployable owns.
const GROUPS: [RouteGroup; 2] = [RouteGroup::Observations, RouteGroup::TelemetryLifecycle];

fn owned() -> Vec<RouteId> {
    GROUPS
        .iter()
        .flat_map(|group| group.routes().iter().copied())
        .filter(|id| route_owner(*id) == Some(RouteOwner::ObservationApi))
        .collect()
}

#[test]
fn this_deployable_owns_the_twenty_seven_finite_observation_routes() {
    assert_eq!(RouteGroup::Observations.routes().len(), 39);
    assert_eq!(RouteGroup::TelemetryLifecycle.routes().len(), 12);
    assert_eq!(owned().len(), 27);
}

#[test]
fn the_groups_partition_cleanly_from_every_other_deployable() {
    for id in RouteId::ALL {
        let mine = owned().contains(id);
        let expected =
            GROUPS.contains(&id.group()) && route_owner(*id) == Some(RouteOwner::ObservationApi);
        assert_eq!(
            mine,
            expected,
            "`{}` disagrees with ownership",
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
fn every_ndjson_route_belongs_exclusively_to_regional_stream() {
    let streaming: Vec<&'static str> = owned()
        .into_iter()
        .filter(|id| route(*id).transport == TransportKind::Ndjson)
        .map(|id| route(id).operation_id)
        .collect();
    assert!(
        streaming.is_empty(),
        "the finite Lambda must mount no NDJSON route"
    );
    assert_eq!(RouteOwner::Stream.routes().len(), 24);
}

#[test]
fn the_dormant_export_admission_contract_still_names_the_inconsistent_operation_shape() {
    for id in [
        RouteId::TelemetryExportCreate,
        RouteId::SessionTelemetryExportCreate,
    ] {
        let descriptor = route(id);
        assert_eq!(descriptor.success_status, 202);
        assert_eq!(
            descriptor.idempotency,
            IdempotencyKind::OperationId,
            "the dormant contract still carries the operation identity that the authority redesign must reconcile"
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
