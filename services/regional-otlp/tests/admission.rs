//! Contract evidence for the OTLP admission edge.
//!
//! These cases hold the two facts a deployment can silently get wrong: which
//! routes this binary owns, and what the registered admission ceilings are. Both
//! are read from the generated table and the registry rather than restated here,
//! so a drift is a failure rather than a second copy that agrees with nobody.

use aex_otlp_admission::OtlpLimits;
use aex_wire::routes::{BodyClass, Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;

#[test]
fn this_deployable_owns_exactly_the_three_otlp_ingest_routes() {
    let owned: Vec<&'static str> = RouteGroup::Otlp
        .routes()
        .iter()
        .map(|id| route(*id).operation_id)
        .collect();
    assert_eq!(
        owned,
        vec![
            "otlp_logs_ingest",
            "otlp_metrics_ingest",
            "otlp_traces_ingest"
        ]
    );
}

#[test]
fn every_owned_route_is_a_unary_regional_otlp_post() {
    for id in RouteGroup::Otlp.routes() {
        let descriptor = route(*id);
        assert_eq!(descriptor.plane, Plane::Regional);
        assert_eq!(descriptor.body_class, BodyClass::Otlp);
        assert_eq!(descriptor.transport, TransportKind::Unary);
        assert!(
            descriptor.template.starts_with("/api/telemetry/otlp/v1/"),
            "{}",
            descriptor.template
        );
    }
}

#[test]
fn the_group_partitions_cleanly_from_every_other_deployable() {
    for id in RouteId::ALL {
        let in_group = RouteGroup::Otlp.routes().contains(id);
        assert_eq!(
            in_group,
            id.group() == RouteGroup::Otlp,
            "`{}` disagrees with its own group",
            route(*id).operation_id
        );
    }
}

#[test]
fn the_registered_ceilings_are_the_ones_this_binary_enforces() {
    let limits = OtlpLimits::REGISTERED;
    assert_eq!(limits.encoded_max, 4 * 1024 * 1024, "4 MiB encoded");
    assert_ne!(
        limits.encoded_max,
        6 * 1024 * 1024,
        "the replaced implementation's 6 MiB is not ported"
    );
    assert_eq!(limits.decoded_max, 16 * 1024 * 1024, "16 MiB decoded");
    assert_eq!(limits.max_records, 2_000, "2000 observations");
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
