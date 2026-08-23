//! The hosted `storage` tool: the former `brain.storage` kernel intrinsic as an ordinary
//! Aex server capability. The model-facing contract is unchanged (save/load/list over the
//! session's durable storage and its root default sandbox); the implementation is plain
//! service code calling Brain's session-scoped storage API with Aex's deployment
//! credentials. Replay safety is layered: the executor memoizes one exact response per
//! `(session_id, call_id)`, and sandbox copies carry the call id as Brain's
//! Idempotency-Key so a replayed effect resolves to the first result.

use std::time::Duration;

use brain_protocol::session::ExternalToolCallRequest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::BrainClient;
use crate::store::Db;
use crate::{Error, Result, now_ms};

pub const STORAGE_CAPABILITY: &str = "aex.storage";
pub const STORAGE_TOOL_NAME: &str = "storage";

const STORAGE_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn execute(
    db: &Db,
    brain: &BrainClient,
    request: ExternalToolCallRequest,
) -> Result<Value> {
    if request.name.as_str() != STORAGE_TOOL_NAME {
        return Err(Error::Invalid(format!(
            "unknown hosted storage Tool {:?}",
            request.name
        )));
    }
    let session_id = request.session_id.to_string();
    let call_id = request.call_id.to_string();
    let row = db
        .session(session_id.clone())
        .await?
        .ok_or_else(|| Error::Invalid("storage call names an unknown hosted session".into()))?;
    let request_hash = hex::encode(Sha256::digest(serde_jcs::to_vec(&request).map_err(
        |error| Error::Internal(format!("storage Tool request canonicalization: {error}")),
    )?));
    if let Some(cached) = db
        .hosted_tool_response(session_id.clone(), call_id.clone(), request_hash.clone())
        .await?
    {
        return serde_json::from_str(&cached)
            .map_err(|error| Error::Internal(format!("cached storage Tool response: {error}")));
    }
    let operation = perform(
        brain,
        &row.account_id,
        &session_id,
        &call_id,
        &request.input,
    );
    let response = match tokio::time::timeout(STORAGE_TIMEOUT, operation).await {
        Err(_) => tool_failure(
            "deadline_exceeded",
            format!(
                "storage exceeded the {} ms host deadline",
                STORAGE_TIMEOUT.as_millis()
            ),
        ),
        Ok(Err(ToolError::Model(message))) => tool_failure("failed", message),
        Ok(Err(ToolError::Host(error))) => return Err(error),
        Ok(Ok(result)) => json!({
            "outcome": "completed",
            "content": result.to_string(),
            "is_error": false,
            "disposition": "continue",
            "result": result,
        }),
    };
    let stored = db
        .record_hosted_tool_response(
            session_id,
            call_id,
            request_hash,
            response.to_string(),
            now_ms(),
        )
        .await?;
    serde_json::from_str(&stored)
        .map_err(|error| Error::Internal(format!("stored storage Tool response: {error}")))
}

/// A model-visible failure (honest tool error content) versus a host failure (the executor
/// itself misbehaved and Brain should see a retryable transport-level error).
enum ToolError {
    Model(String),
    Host(Error),
}

impl From<Error> for ToolError {
    fn from(error: Error) -> Self {
        ToolError::Host(error)
    }
}

async fn perform(
    brain: &BrainClient,
    tenant_id: &str,
    session_id: &str,
    call_id: &str,
    input: &Value,
) -> std::result::Result<Value, ToolError> {
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Model("storage action is required".into()))?;
    match action {
        "list" => {
            let body = json!({
                "prefix": input.get("prefix"),
                "cursor": input.get("cursor"),
                "limit": input.get("limit"),
            });
            brain_json(
                brain,
                tenant_id,
                &format!("/v1/sessions/{session_id}/storage/list"),
                None,
                body,
            )
            .await
        }
        "save" => {
            let source = input
                .get("source")
                .ok_or_else(|| ToolError::Model("storage save source is required".into()))?;
            match source.get("kind").and_then(Value::as_str) {
                Some("inline_text") => {
                    let text = source
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ToolError::Model("storage save text is required".into()))?;
                    use base64::Engine as _;
                    let body = json!({
                        "key": input.get("key"),
                        "content_base64":
                            base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
                        "content_type": "text/plain; charset=utf-8",
                        "overwrite": input.get("overwrite").and_then(Value::as_bool).unwrap_or(false),
                    });
                    brain_json(
                        brain,
                        tenant_id,
                        &format!("/v1/sessions/{session_id}/storage/write-inline"),
                        None,
                        body,
                    )
                    .await
                }
                Some("sandbox_path") => {
                    let body = json!({
                        "key": input.get("key"),
                        "path": source.get("path"),
                        "sandbox_generation": source.get("generation"),
                        "overwrite": input.get("overwrite").and_then(Value::as_bool).unwrap_or(false),
                    });
                    brain_json(
                        brain,
                        tenant_id,
                        &format!("/v1/sessions/{session_id}/storage/copy-from-sandbox"),
                        Some(call_id),
                        body,
                    )
                    .await
                }
                other => Err(ToolError::Model(format!(
                    "unknown storage save source kind {other:?}"
                ))),
            }
        }
        "load" => {
            let body = json!({
                "key": input.get("key"),
                "path": input.get("path"),
                "sandbox_generation": input.get("generation"),
                "overwrite": input.get("overwrite").and_then(Value::as_bool).unwrap_or(false),
            });
            brain_json(
                brain,
                tenant_id,
                &format!("/v1/sessions/{session_id}/storage/copy-to-sandbox"),
                Some(call_id),
                body,
            )
            .await
        }
        other => Err(ToolError::Model(format!(
            "unknown storage action {other:?}"
        ))),
    }
}

/// One session-scoped Brain call. Brain 4xx bodies become honest model-visible failures;
/// 5xx and transport losses stay host errors so Brain's executor adapter surfaces a
/// retryable failure instead of caching a spurious terminal.
async fn brain_json(
    brain: &BrainClient,
    tenant_id: &str,
    path: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> std::result::Result<Value, ToolError> {
    let response = brain
        .forward(
            tenant_id,
            reqwest::Method::POST,
            path,
            Some("application/json"),
            idempotency_key,
            Some(body.to_string().into()),
            Some(STORAGE_TIMEOUT),
        )
        .await
        .map_err(ToolError::Host)?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ToolError::Host(Error::Upstream(format!("storage body: {error}"))))?;
    if (200..300).contains(&status) {
        return serde_json::from_slice(&bytes)
            .map_err(|error| ToolError::Host(Error::Upstream(format!("storage JSON: {error}"))));
    }
    let message = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("storage operation failed with status {status}"));
    if (400..500).contains(&status) {
        Err(ToolError::Model(message))
    } else {
        Err(ToolError::Host(Error::Upstream(format!(
            "storage -> {status}: {message}"
        ))))
    }
}

fn tool_failure(outcome: &str, message: String) -> Value {
    json!({
        "outcome": outcome,
        "content": message,
        "is_error": true,
        "disposition": "continue",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rating::FoldState;
    use crate::store::SessionRow;

    fn request(input: Value) -> ExternalToolCallRequest {
        serde_json::from_value(json!({
            "session_id": "ses_storagetool0123456789",
            "turn_id": "trn_storagetool0123456789",
            "agent_id": "root",
            "call_id": "op_storagetool0123456789",
            "name": "storage",
            "input": input,
            "context": {"brain.capability": STORAGE_CAPABILITY},
        }))
        .expect("executor request")
    }

    async fn db_with_session() -> Db {
        let db = Db::open_memory().expect("memory db");
        db.create_account(
            crate::store::AccountRow {
                id: "acc_storage_test".into(),
                email: "storage@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "storage@example.test".into(),
            "storage-token-hash".into(),
        )
        .await
        .expect("account row");
        db.insert_session(SessionRow {
            id: "ses_storagetool0123456789".into(),
            account_id: "acc_storage_test".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_storagetool0123456789".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: FoldState::default(),
        })
        .await
        .expect("session row");
        db
    }

    #[tokio::test]
    async fn an_unknown_session_is_refused_before_any_brain_call() {
        let db = Db::open_memory().expect("memory db");
        let brain = BrainClient::new("http://127.0.0.1:1", "test-operator");
        let error = execute(&db, &brain, request(json!({"action": "list"})))
            .await
            .expect_err("unknown session");
        assert!(matches!(error, Error::Invalid(_)), "{error:?}");
    }

    #[tokio::test]
    async fn an_invalid_action_fails_honestly_and_memoizes_one_exact_response() {
        let db = db_with_session().await;
        let brain = BrainClient::new("http://127.0.0.1:1", "test-operator");
        let first = execute(&db, &brain, request(json!({"action": "explode"})))
            .await
            .expect("model-visible failure");
        assert_eq!(first["outcome"], "failed");
        assert_eq!(first["is_error"], true);
        assert!(
            first["content"]
                .as_str()
                .unwrap()
                .contains("unknown storage action"),
            "{first}"
        );
        let replay = execute(&db, &brain, request(json!({"action": "explode"})))
            .await
            .expect("memoized replay");
        assert_eq!(replay, first);
        let reused = execute(&db, &brain, request(json!({"action": "list"})))
            .await
            .expect_err("reused call identity with a different request");
        assert!(matches!(reused, Error::Conflict(_)), "{reused:?}");
    }
}
