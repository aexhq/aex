//! Bounded local ingress performance contract; no external resource is created.

use aex_tool_mux::{TelemetryEnvelope, TelemetryEvent, TelemetryKind, TelemetryPort as _};
use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};

#[tokio::test]
async fn ten_thousand_events_cross_the_bounded_handoff_without_growth() {
    let (ingress, mut receiver) = tool_mux::telemetry::BoundedTelemetryIngress::new(1);
    for ordinal in 1_u64..=10_000 {
        let envelope = TelemetryEnvelope {
            producer_ordinal: ordinal,
            gap_before: None,
            event: TelemetryEvent {
                session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
                hand: None,
                generation: None,
                call: None,
                kind: TelemetryKind::RemoteMcpStarted,
            },
        };
        ingress.try_emit(envelope).expect("one open slot");
        assert_eq!(
            receiver.recv().await.expect("accepted").producer_ordinal,
            ordinal
        );
    }
}
