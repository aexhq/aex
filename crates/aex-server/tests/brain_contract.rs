use aex_server::brain_contract::{CreateSession, ErrorResponse, EventList, ExecutorRegistration};
use serde_json::json;

#[test]
fn generated_admission_preserves_private_credentials_and_rejects_old_contracts() {
    let request: CreateSession = serde_json::from_value(json!({
        "harness":"pi", "config":{"model":"openai/gpt-4.1-mini","tools":[]},
        "model_credential":{"api_key":"private-key"}
    }))
    .unwrap();
    assert_eq!(
        request.model_credential.unwrap().api_key.as_deref(),
        Some("private-key")
    );
    assert!(
        serde_json::from_value::<CreateSession>(json!({"agentloop":{"name":"pi"},"model":{}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<CreateSession>(
            json!({"harness":"pi","config":{"model":"openai/model","tools":[]},"tenant":"foreign"})
        )
        .is_err()
    );
}

#[test]
fn generated_events_errors_and_executors_preserve_opaque_identity() {
    let page: EventList = serde_json::from_value(json!({"events":[{
        "id":"input", "type":"session.status", "time":"2026-10-20T00:00:00Z", "input_id":"input", "cursor":"opaque+/=", "data":{"status":"idle"}
    }],"next_cursor":"opaque+/="})).unwrap();
    assert_eq!(page.events[0].input_id.as_deref(), Some("input"));
    assert_eq!(page.next_cursor, "opaque+/=");
    let error: ErrorResponse = serde_json::from_value(
        json!({"error":{"code":"conflict","message":"different input"},"request_id":"request"}),
    )
    .unwrap();
    assert_eq!(error.error.code, "conflict");
    let registration: ExecutorRegistration =
        serde_json::from_value(json!({"binding":"runtime","tools":[]})).unwrap();
    assert!(registration.session.is_none());
}
