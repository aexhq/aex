//! Generated ownership is the complete public contract of `session-api`.
//!
//! Handler behavior belongs to the focused session, registry, upload, and
//! telemetry suites. This target prevents the deployable composition from
//! quietly growing a second route list.

use aex_regional_http::router::RouteOwner;
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use session_stream_api::session::handlers::{Routes, served_groups};

const EXPECTED: [RouteId; 19] = [
    RouteId::RegistryFilesDelete,
    RouteId::RegistryFilesDownloadCreate,
    RouteId::RegistryFilesGet,
    RouteId::RegistryFilesList,
    RouteId::RegistryFilesPut,
    RouteId::SessionCancel,
    RouteId::SessionCreate,
    RouteId::SessionDelete,
    RouteId::SessionGet,
    RouteId::SessionMessageSend,
    RouteId::SessionMessagesList,
    RouteId::SessionMessagesStream,
    RouteId::SessionTelemetryDownloadCreate,
    RouteId::SessionTelemetryReplay,
    RouteId::SessionTelemetryStream,
    RouteId::SessionTerminate,
    RouteId::SessionsList,
    RouteId::UploadComplete,
    RouteId::UploadCreate,
];

#[test]
fn served_routes_are_exactly_the_generated_nineteen_route_contract() {
    assert_eq!(RouteOwner::SessionApi.routes(), EXPECTED);
    assert_eq!(Routes::served(), EXPECTED);
    assert!(EXPECTED.iter().all(|id| {
        let descriptor = route(*id);
        descriptor.plane == Plane::Regional && !descriptor.deferred
    }));
}

#[test]
fn the_contract_has_sixteen_finite_and_three_streaming_routes() {
    let finite = EXPECTED
        .iter()
        .filter(|id| {
            matches!(
                route(**id).transport,
                TransportKind::Unary | TransportKind::Binary
            )
        })
        .count();
    let streaming = EXPECTED
        .iter()
        .filter(|id| route(**id).transport == TransportKind::Ndjson)
        .count();

    assert_eq!((finite, streaming), (16, 3));
}

#[test]
fn the_composition_draws_only_from_registry_sessions_and_uploads() {
    assert_eq!(
        served_groups(),
        vec![
            RouteGroup::Registry,
            RouteGroup::Sessions,
            RouteGroup::Uploads,
        ]
    );
}
