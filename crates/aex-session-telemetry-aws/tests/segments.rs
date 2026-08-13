//! Security and wire-shape tests for immutable session telemetry segments.

use aex_session_telemetry_aws::{SessionEvent, SessionOutcome, encode};
use aex_wire::types::Timestamp;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use prost::Message as _;

fn event() -> SessionEvent {
    SessionEvent::message_completed(
        42,
        SessionOutcome::Succeeded,
        Timestamp::from_unix_millis(1_800_000_000_000).expect("fixture timestamp"),
    )
}

#[test]
fn encodes_one_allowlisted_official_otlp_log() {
    let encoded = encode(event()).expect("closed event encodes");
    let request = ExportLogsServiceRequest::decode(encoded.bytes.as_slice())
        .expect("official generated type decodes its bytes");
    assert_eq!(request.resource_logs.len(), 1);
    let resource_logs = &request.resource_logs[0];
    assert_eq!(resource_logs.scope_logs.len(), 1);
    let records = &resource_logs.scope_logs[0].log_records;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert!(record.trace_id.is_empty());
    assert!(record.span_id.is_empty());
    assert_eq!(record.attributes.len(), 3);
    let keys = record
        .attributes
        .iter()
        .map(|attribute| attribute.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "aex.session.event",
            "aex.session.outcome",
            "aex.session.sequence"
        ]
    );
    assert!(matches!(
        record.body.as_ref().and_then(|value| value.value.as_ref()),
        Some(any_value::Value::StringValue(value)) if value == "aex.session.message.completed"
    ));
}

#[test]
fn content_hash_and_id_are_deterministic() {
    let first = encode(event()).expect("first encode");
    let replay = encode(event()).expect("replay encode");
    assert_eq!(first, replay);
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(first.id, format!("00000000000000000042-{}", first.sha256));
}

#[test]
fn sensitive_application_and_private_tracing_values_cannot_enter_bytes() {
    let forbidden = [
        "PROMPT-canary-e89e7a",
        "COMPLETION-canary-dd4a11",
        "TOOL-ARGS-canary-90abef",
        "TOOL-RESULT-canary-4455cc",
        "INTERNAL-RUN-ID-canary-168d12",
        "PRIVATE-TRACE-ID-canary-a90341",
        "PRIVATE-SPAN-ID-canary-c156a0",
    ];
    // `SessionEvent` has no field through which any value above can be passed.
    let encoded = encode(event()).expect("closed event encodes");
    for sentinel in forbidden {
        assert!(
            !encoded
                .bytes
                .windows(sentinel.len())
                .any(|window| window == sentinel.as_bytes()),
            "forbidden sentinel appeared in encoded bytes: {sentinel}"
        );
    }
}
