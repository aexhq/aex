//! Nonblocking failure behavior of the service telemetry ingress.

use aex_tool_mux::{
    TelemetryEnvelope, TelemetryEvent, TelemetryKind, TelemetryPort as _, TelemetryPressure,
};
use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};

#[test]
fn a_closed_exporter_fails_open_immediately() {
    let (ingress, receiver) = tool_mux::telemetry::BoundedTelemetryIngress::new(1);
    drop(receiver);
    let envelope = TelemetryEnvelope {
        producer_ordinal: 1,
        gap_before: None,
        event: TelemetryEvent {
            session: SessionId::from_uuid7(Uuid7::compose(1, [2; 10])),
            hand: None,
            generation: None,
            call: None,
            kind: TelemetryKind::RemoteMcpStarted,
        },
    };
    assert_eq!(ingress.try_emit(envelope), Err(TelemetryPressure::Closed));
}
