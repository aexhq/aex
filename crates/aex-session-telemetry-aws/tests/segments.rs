//! Security and wire-shape tests for immutable session telemetry segments.

use aex_session_telemetry_aws::{decode_encoded, encode_frames};
use aex_wire::ids::{PrefixedId as _, SessionId, SpanId, TraceId, Uuid7};
use aex_wire::models::{TelemetryFrame, TelemetryKind};
use aex_wire::types::DecimalU128;
use aex_wire::types::Timestamp;

fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn log_frame() -> TelemetryFrame {
    TelemetryFrame {
        sequence: DecimalU128::new(42),
        kind: TelemetryKind::Log,
        occurred_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("time"),
        trace_id: None,
        span_id: None,
        body: None,
        preview: None,
        truncated: false,
    }
}

#[test]
fn encodes_one_allowlisted_official_otlp_log() {
    let frame = log_frame();
    let encoded = encode_frames(session(), std::slice::from_ref(&frame)).expect("encodes");
    assert_eq!(
        decode_encoded(session(), &encoded).expect("decodes"),
        vec![frame]
    );
}

#[test]
fn content_hash_and_id_are_deterministic() {
    let frame = log_frame();
    let first = encode_frames(session(), std::slice::from_ref(&frame)).expect("first encode");
    let replay = encode_frames(session(), std::slice::from_ref(&frame)).expect("replay encode");
    assert_eq!(first, replay);
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(
        first.id,
        format!("00000000000000000042-00000000000000000042-{}", first.sha256)
    );
}

#[test]
fn official_otlp_span_round_trips_with_w3c_correlation() {
    let frame = TelemetryFrame {
        sequence: DecimalU128::new(9),
        kind: TelemetryKind::Span,
        occurred_at: Timestamp::from_unix_millis(1_800_000_000_100).expect("time"),
        trace_id: Some(TraceId::from_bytes([1; 16]).expect("trace")),
        span_id: Some(SpanId::from_bytes([2; 8]).expect("span")),
        body: None,
        preview: None,
        truncated: false,
    };
    let encoded = encode_frames(session(), std::slice::from_ref(&frame)).expect("encodes");
    assert!(encoded.bytes.len() < 1024);
    assert_eq!(
        decode_encoded(session(), &encoded).expect("decodes"),
        vec![frame]
    );
}
