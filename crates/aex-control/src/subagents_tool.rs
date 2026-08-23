//! The hosted `subagents` tool: the former `brain.subagents` kernel intrinsic as an
//! ordinary Aex server capability. The intrinsic already spoke only the public
//! child-session verbs; this executor speaks the same verbs over Brain's session-scoped
//! children API with Aex's deployment credentials. Spawn and message sends carry the
//! call id as Brain's Idempotency-Key, and — like every hosted capability — one exact
//! response is memoized per `(session_id, call_id)` so journal replays stay stable.

use std::time::Duration;

use brain_protocol::session::ExternalToolCallRequest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::BrainClient;
use crate::storage_tool::{ToolError, brain_json, tool_failure};
use crate::store::Db;
use crate::{Error, Result, now_ms};

pub const SUBAGENTS_CAPABILITY: &str = "aex.subagents";
pub const SUBAGENTS_TOOL_NAME: &str = "subagents";

/// The contract's wait ceiling is 300 s; the host deadline leaves margin for transport.
const WAIT_CEILING_MS: u64 = 300_000;
const SUBAGENTS_TIMEOUT: Duration = Duration::from_secs(330);

pub async fn execute(
    db: &Db,
    brain: &BrainClient,
    request: ExternalToolCallRequest,
) -> Result<Value> {
    if request.name.as_str() != SUBAGENTS_TOOL_NAME {
        return Err(Error::Invalid(format!(
            "unknown hosted subagents Tool {:?}",
            request.name
        )));
    }
    let session_id = request.session_id.to_string();
    let call_id = request.call_id.to_string();
    let row = db
        .session(session_id.clone())
        .await?
        .ok_or_else(|| Error::Invalid("subagents call names an unknown hosted session".into()))?;
    let request_hash = hex::encode(Sha256::digest(serde_jcs::to_vec(&request).map_err(
        |error| Error::Internal(format!("subagents Tool request canonicalization: {error}")),
    )?));
    if let Some(cached) = db
        .hosted_tool_response(session_id.clone(), call_id.clone(), request_hash.clone())
        .await?
    {
        return serde_json::from_str(&cached)
            .map_err(|error| Error::Internal(format!("cached subagents Tool response: {error}")));
    }
    let operation = perform(
        brain,
        &row.account_id,
        &session_id,
        &call_id,
        &request.input,
    );
    let response = match tokio::time::timeout(SUBAGENTS_TIMEOUT, operation).await {
        Err(_) => tool_failure(
            "deadline_exceeded",
            format!(
                "subagents exceeded the {} ms host deadline",
                SUBAGENTS_TIMEOUT.as_millis()
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
        .map_err(|error| Error::Internal(format!("stored subagents Tool response: {error}")))
}

fn required_str<'a>(input: &'a Value, field: &str) -> std::result::Result<&'a str, ToolError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::Model(format!("subagents {field} is required")))
}

fn child_path(session_id: &str, child_id: &str, suffix: &str) -> String {
    format!("/v1/sessions/{session_id}/children/{child_id}{suffix}")
}

async fn perform(
    brain: &BrainClient,
    tenant_id: &str,
    session_id: &str,
    call_id: &str,
    input: &Value,
) -> std::result::Result<Value, ToolError> {
    use reqwest::Method;
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Model("subagents action is required".into()))?;
    match action {
        "spawn_agent" => {
            let body = json!({
                "prompt": required_str(input, "message")?,
                "name": required_str(input, "task_name")?,
                "fork_turns": input.get("fork_turns"),
            });
            brain_json(
                brain,
                tenant_id,
                Method::POST,
                &format!("/v1/sessions/{session_id}/children"),
                Some(call_id),
                Some(body),
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "send_message" | "follow_up" => {
            let child_id = required_str(input, "child_id")?;
            let body = json!({ "message": required_str(input, "message")? });
            brain_json(
                brain,
                tenant_id,
                Method::POST,
                &child_path(session_id, child_id, "/messages"),
                Some(call_id),
                Some(body),
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "peek" => {
            let child_id = required_str(input, "child_id")?;
            brain_json(
                brain,
                tenant_id,
                Method::GET,
                &child_path(session_id, child_id, ""),
                None,
                None,
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "wait" => {
            let child_id = required_str(input, "child_id")?;
            let timeout_ms = input
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(30_000)
                .min(WAIT_CEILING_MS);
            brain_json(
                brain,
                tenant_id,
                Method::POST,
                &child_path(session_id, child_id, "/wait"),
                None,
                Some(json!({ "timeout_ms": timeout_ms })),
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "list_children" => {
            let mut query = String::new();
            if let Some(cursor) = input.get("cursor").and_then(Value::as_str) {
                query = format!("?cursor={}", urlencoding::encode(cursor));
            }
            if let Some(limit) = input.get("limit").and_then(Value::as_u64) {
                let separator = if query.is_empty() { "?" } else { "&" };
                query = format!("{query}{separator}limit={}", limit.min(100));
            }
            brain_json(
                brain,
                tenant_id,
                Method::GET,
                &format!("/v1/sessions/{session_id}/children{query}"),
                None,
                None,
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "interrupt_agent" => {
            let child_id = required_str(input, "child_id")?;
            brain_json(
                brain,
                tenant_id,
                Method::POST,
                &child_path(session_id, child_id, "/interrupt"),
                None,
                Some(json!({})),
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        "end_agent" => {
            let child_id = required_str(input, "child_id")?;
            brain_json(
                brain,
                tenant_id,
                Method::POST,
                &child_path(session_id, child_id, "/end"),
                None,
                Some(json!({})),
                SUBAGENTS_TIMEOUT,
            )
            .await
        }
        other => Err(ToolError::Model(format!(
            "unknown subagents action {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rating::FoldState;
    use crate::store::SessionRow;

    fn request(input: Value) -> ExternalToolCallRequest {
        serde_json::from_value(json!({
            "session_id": "ses_subagenttool012345678",
            "turn_id": "trn_subagenttool012345678",
            "agent_id": "root",
            "call_id": "op_subagenttool0123456789",
            "name": "subagents",
            "input": input,
            "context": {"brain.capability": SUBAGENTS_CAPABILITY},
        }))
        .expect("executor request")
    }

    async fn db_with_session() -> Db {
        let db = Db::open_memory().expect("memory db");
        db.create_account(
            crate::store::AccountRow {
                id: "acc_subagents_test".into(),
                email: "subagents@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "subagents@example.test".into(),
            "subagents-token-hash".into(),
        )
        .await
        .expect("account row");
        db.insert_session(SessionRow {
            id: "ses_subagenttool012345678".into(),
            account_id: "acc_subagents_test".into(),
            key_id: "key_1".into(),
            parent_id: None,
            root_id: "ses_subagenttool012345678".into(),
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
    async fn an_invalid_action_fails_honestly_and_memoizes() {
        let db = db_with_session().await;
        let brain = BrainClient::new("http://127.0.0.1:1", "test-operator");
        let first = execute(&db, &brain, request(json!({"action": "explode"})))
            .await
            .expect("model-visible failure");
        assert_eq!(first["outcome"], "failed");
        assert!(
            first["content"]
                .as_str()
                .unwrap()
                .contains("unknown subagents action"),
            "{first}"
        );
        let replay = execute(&db, &brain, request(json!({"action": "explode"})))
            .await
            .expect("memoized replay");
        assert_eq!(replay, first);
    }

    #[tokio::test]
    async fn a_missing_child_id_is_a_model_visible_failure() {
        let db = db_with_session().await;
        let brain = BrainClient::new("http://127.0.0.1:1", "test-operator");
        let response = execute(&db, &brain, request(json!({"action": "peek"})))
            .await
            .expect("model-visible failure");
        assert_eq!(response["is_error"], true);
        assert!(
            response["content"].as_str().unwrap().contains("child_id"),
            "{response}"
        );
    }
}
