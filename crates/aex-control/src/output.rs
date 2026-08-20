//! Per-message typed output orchestration owned by the trusted control plane.
//!
//! The model sees one stable, create-time tool and a normal tail instruction containing the
//! per-message schema. Candidate validation runs here, never in the customer's hand. Brain only
//! knows the generic external-tool and return-direct contracts.

use aex_contracts::session::ExternalToolCallRequest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::identity;
use crate::store::{BeginOutput, Db, OutputRequestRow};
use crate::{Error, Result, now_ms};

pub const OUTPUT_TOOL_NAME: &str = "aex_submit_output";
pub const OUTPUT_CONTEXT_KEY: &str = "aex.output_request_id";
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 96 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 4096;
const MAX_PATTERN_CHARS: usize = 1024;
const MAX_ISSUES: usize = 20;

pub enum PreparedMessage {
    Plain(Vec<u8>),
    New {
        body: Vec<u8>,
        output_id: String,
        schema_hash: String,
    },
    Replay {
        accepted_json: String,
    },
}

/// Hosted Aex owns this executor and therefore reserves the entire external-tool namespace on
/// public creates for the MVP. Direct Brain deployments can still compose arbitrary declarations.
pub fn inject_output_tool(body: &[u8]) -> Result<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body)
        .map_err(|error| Error::Invalid(format!("session body: {error}")))?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("session body must be an object".into()))?;
    let tools = root.entry("tools").or_insert_with(|| json!({}));
    let tools = tools
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("tools must be an object".into()))?;
    if tools
        .get("external")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
    {
        return Err(Error::Invalid(
            "tools.external is reserved by hosted Aex in the MVP".into(),
        ));
    }
    tools.insert(
        "external".into(),
        json!([{
            "name": OUTPUT_TOOL_NAME,
            "description": "Submit the final structured result requested by the latest user message. Call this once with exactly the result object; do not wrap it in another property.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "additionalProperties": true
            },
            "scope": "root",
            "completion": "return_direct",
            "effect": "replay_safe",
            "max_input_bytes": MAX_OUTPUT_BYTES
        }]),
    );
    serde_json::to_vec(&document).map_err(|error| Error::Internal(format!("session body: {error}")))
}

pub async fn prepare_message(
    db: &Db,
    session_id: &str,
    output_capable: bool,
    body: &[u8],
    idempotency_key: Option<&str>,
) -> Result<PreparedMessage> {
    let mut document: Value = serde_json::from_slice(body)
        .map_err(|error| Error::Invalid(format!("message body: {error}")))?;
    let request_hash = canonical_hash(&document)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("message body must be an object".into()))?;
    let Some(output) = root.remove("output") else {
        return Ok(PreparedMessage::Plain(body.to_vec()));
    };
    if !output_capable {
        return Err(Error::Conflict(
            "this session predates typed output; create a new session before using send(..., { output })"
                .into(),
        ));
    }
    let output = output
        .as_object()
        .ok_or_else(|| Error::Invalid("output must be an object".into()))?;
    let schema = output
        .get("schema")
        .ok_or_else(|| Error::Invalid("output.schema is required".into()))?;
    let schema_hash = output
        .get("schema_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("output.schema_hash is required".into()))?;
    let retries = output.get("retries").and_then(Value::as_i64).unwrap_or(1);
    if !(0..=2).contains(&retries) {
        return Err(Error::Invalid("output.retries must be 0, 1, or 2".into()));
    }
    let schema_json = validate_schema(schema, schema_hash)?;

    let idempotency_key_hash = match idempotency_key {
        Some(key) if key.is_empty() || key.len() > 128 => {
            return Err(Error::Invalid(
                "Idempotency-Key must contain 1 to 128 bytes".into(),
            ));
        }
        Some(key) => Some(identity::hash_secret(key)),
        None => None,
    };
    let output_id = identity::new_id("out");
    let metadata = root.entry("metadata").or_insert_with(|| json!({}));
    let metadata = metadata
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("metadata must be an object".into()))?;
    if metadata.keys().any(|key| key.starts_with("aex.")) {
        return Err(Error::Invalid(
            "message metadata keys beginning with aex. are reserved".into(),
        ));
    }
    metadata.insert(OUTPUT_CONTEXT_KEY.into(), Value::String(output_id.clone()));

    let instruction = format!(
        "Aex structured-output request. Before ending this turn, call `{OUTPUT_TOOL_NAME}` exactly once with the final result object. The call argument must validate against this JSON Schema. If the tool reports validation errors, correct the object and call it again. Do not merely print JSON.\n\nJSON Schema (RFC 8785 canonical form):\n{schema_json}"
    );
    match root.get_mut("content") {
        Some(Value::String(content)) => {
            content.push_str("\n\n");
            content.push_str(&instruction);
        }
        Some(Value::Array(parts)) => parts.push(json!({"type": "text", "text": instruction})),
        _ => return Err(Error::Invalid("message.content is required".into())),
    }
    let body = serde_json::to_vec(&document)
        .map_err(|error| Error::Internal(format!("message body: {error}")))?;
    let row = OutputRequestRow {
        id: output_id,
        session_id: session_id.to_string(),
        schema_hash: schema_hash.to_string(),
        schema_json: schema_json.clone(),
        max_attempts: retries + 1,
        attempts: 0,
        status: "pending".into(),
        turn_id: None,
        accepted_json: None,
        result_json: None,
        error_json: None,
        idempotency_key_hash,
        request_hash,
        created_ms: now_ms(),
    };
    let row = match db.begin_output(row).await? {
        BeginOutput::Created(row) => row,
        BeginOutput::Existing(row) => {
            return match row.accepted_json {
                Some(accepted_json) => Ok(PreparedMessage::Replay { accepted_json }),
                None => Err(Error::Conflict(
                    "the matching output request is still being admitted".into(),
                )),
            };
        }
        BeginOutput::RequestMismatch => {
            return Err(Error::Conflict(
                "Idempotency-Key was already used with a different request".into(),
            ));
        }
    };

    Ok(PreparedMessage::New {
        body,
        output_id: row.id,
        schema_hash: row.schema_hash,
    })
}

pub fn augment_accepted(
    bytes: &[u8],
    output_id: &str,
    schema_hash: &str,
) -> Result<(String, String)> {
    let mut accepted: Value = serde_json::from_slice(bytes)
        .map_err(|error| Error::Upstream(format!("message accepted body: {error}")))?;
    let object = accepted
        .as_object_mut()
        .ok_or_else(|| Error::Upstream("message accepted body is not an object".into()))?;
    let turn_id = object
        .get("turn_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Upstream("message accepted body has no turn_id".into()))?
        .to_string();
    object.insert("output_id".into(), Value::String(output_id.to_string()));
    object.insert("schema_hash".into(), Value::String(schema_hash.to_string()));
    Ok((turn_id, accepted.to_string()))
}

pub async fn execute(db: &Db, request: ExternalToolCallRequest) -> Result<Value> {
    let session_id = request.session_id.to_string();
    let call_id = request.call_id.to_string();
    if request.name != OUTPUT_TOOL_NAME {
        return Err(Error::Invalid(format!(
            "unknown hosted external tool {:?}",
            request.name
        )));
    }
    if let Some(response) = db
        .external_call_response(session_id.clone(), call_id.clone())
        .await?
    {
        return serde_json::from_str(&response)
            .map_err(|error| Error::Internal(format!("cached executor response: {error}")));
    }
    let output_id = request
        .context
        .get(OUTPUT_CONTEXT_KEY)
        .ok_or_else(|| Error::Invalid("structured output is not armed for this turn".into()))?
        .clone();
    let row = db
        .output_request(output_id.clone())
        .await?
        .filter(|row| row.session_id == session_id)
        .ok_or(Error::NotFound)?;
    if row.status != "pending" {
        return Err(Error::Conflict(format!(
            "output request {} is already {}",
            row.id, row.status
        )));
    }

    let schema: Value = serde_json::from_str(&row.schema_json)
        .map_err(|error| Error::Internal(format!("stored output schema: {error}")))?;
    let candidate_bytes = serde_jcs::to_vec(&request.input)
        .map_err(|error| Error::Internal(format!("candidate canonicalization: {error}")))?;
    let mut issues = if candidate_bytes.len() > MAX_OUTPUT_BYTES {
        vec![json!({
            "path": "",
            "message": format!("result is {} bytes; maximum is {MAX_OUTPUT_BYTES}", candidate_bytes.len()),
            "keyword": "maxBytes"
        })]
    } else {
        validation_issues(&schema, &request.input)?
    };
    issues.truncate(MAX_ISSUES);
    let attempt = row.attempts + 1;
    let (response, status, result_json, error_json) = if issues.is_empty() {
        let response = json!({
            "outcome": "completed",
            "content": "Structured output accepted.",
            "is_error": false,
            "disposition": "complete_turn",
            "result": request.input,
            "result_metadata": {
                "output_id": row.id,
                "schema_hash": row.schema_hash
            }
        });
        (
            response,
            "completed".to_string(),
            Some(
                String::from_utf8(candidate_bytes).map_err(|error| {
                    Error::Internal(format!("candidate canonical JSON: {error}"))
                })?,
            ),
            None,
        )
    } else if attempt < row.max_attempts {
        (
            json!({
                "outcome": "failed",
                "content": format!(
                    "The submitted object did not match the requested schema. Correct it and call `{OUTPUT_TOOL_NAME}` again. Validation issues: {}",
                    serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into())
                ),
                "is_error": true,
                "disposition": "continue"
            }),
            "pending".to_string(),
            None,
            Some(json!({"issues": issues}).to_string()),
        )
    } else {
        let error = json!({
            "code": "output_validation_error",
            "message": format!(
                "Model output did not satisfy the schema after {attempt} attempt(s)"
            ),
            "details": {"issues": issues}
        });
        (
            json!({
                "outcome": "failed",
                "content": "Structured output validation failed and the configured attempt limit was reached.",
                "is_error": true,
                "disposition": "fail_turn",
                "error": error
            }),
            "failed".to_string(),
            None,
            Some(error.to_string()),
        )
    };
    let response_json = response.to_string();
    let stored = db
        .record_external_attempt(
            session_id,
            call_id,
            output_id,
            response_json,
            status,
            result_json,
            error_json,
            now_ms(),
        )
        .await?;
    serde_json::from_str(&stored)
        .map_err(|error| Error::Internal(format!("executor response: {error}")))
}

fn validate_schema(schema: &Value, claimed_hash: &str) -> Result<String> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::Invalid(
            "output.schema must have an object root".into(),
        ));
    }
    let canonical = serde_jcs::to_vec(schema)
        .map_err(|error| Error::Invalid(format!("output.schema: {error}")))?;
    if canonical.len() > MAX_SCHEMA_BYTES {
        return Err(Error::Invalid(format!(
            "output.schema is {} bytes; maximum is {MAX_SCHEMA_BYTES}",
            canonical.len()
        )));
    }
    let actual = hex::encode(Sha256::digest(&canonical));
    if actual != claimed_hash {
        return Err(Error::Invalid(format!(
            "output.schema_hash mismatch; expected {actual}"
        )));
    }
    let mut nodes = 0;
    inspect_schema(schema, 0, &mut nodes)?;
    jsonschema::meta::validate(schema)
        .map_err(|error| Error::Invalid(format!("output.schema: {error}")))?;
    jsonschema::draft202012::new(schema)
        .map_err(|error| Error::Invalid(format!("output.schema: {error}")))?;
    String::from_utf8(canonical)
        .map_err(|error| Error::Internal(format!("canonical output schema: {error}")))
}

fn inspect_schema(value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(Error::Invalid(format!(
            "output.schema exceeds depth {MAX_SCHEMA_DEPTH}"
        )));
    }
    *nodes += 1;
    if *nodes > MAX_SCHEMA_NODES {
        return Err(Error::Invalid(format!(
            "output.schema exceeds {MAX_SCHEMA_NODES} nodes"
        )));
    }
    match value {
        Value::Object(object) => {
            for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
                if let Some(reference) = object.get(keyword).and_then(Value::as_str)
                    && !reference.starts_with('#')
                {
                    return Err(Error::Invalid(format!(
                        "output.schema remote {keyword} values are not supported"
                    )));
                }
            }
            if let Some(pattern) = object.get("pattern").and_then(Value::as_str)
                && pattern.chars().count() > MAX_PATTERN_CHARS
            {
                return Err(Error::Invalid(format!(
                    "output.schema pattern exceeds {MAX_PATTERN_CHARS} characters"
                )));
            }
            for child in object.values() {
                inspect_schema(child, depth + 1, nodes)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                inspect_schema(child, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validation_issues(schema: &Value, candidate: &Value) -> Result<Vec<Value>> {
    let validator = jsonschema::draft202012::new(schema)
        .map_err(|error| Error::Internal(format!("stored output schema: {error}")))?;
    Ok(validator
        .iter_errors(candidate)
        .take(MAX_ISSUES)
        .map(|error| {
            json!({
                "path": error.instance_path().to_string(),
                "message": error.to_string(),
                "keyword": error.kind().keyword().to_string()
            })
        })
        .collect())
}

fn canonical_hash(value: &Value) -> Result<String> {
    let bytes = serde_jcs::to_vec(value)
        .map_err(|error| Error::Invalid(format!("request canonicalization: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "ses_12345678901234567890";

    async fn db_with_session(output_capable: bool) -> Db {
        let db = Db::open_memory().unwrap();
        db.create_account(
            crate::store::AccountRow {
                id: "acc_output".into(),
                email: "output@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "output@example.test".into(),
            "output-token-hash".into(),
        )
        .await
        .unwrap();
        db.insert_session(crate::store::SessionRow {
            id: SESSION_ID.into(),
            account_id: "acc_output".into(),
            key_id: "key_output".into(),
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            output_capable,
            fold: crate::rating::FoldState::default(),
        })
        .await
        .unwrap();
        db
    }

    #[test]
    fn injection_is_stable_and_rejects_customer_external_tools() {
        let injected = inject_output_tool(br#"{"model":{"provider":"openai"}}"#).unwrap();
        let document: Value = serde_json::from_slice(&injected).unwrap();
        assert_eq!(document["tools"]["external"][0]["name"], OUTPUT_TOOL_NAME);
        assert!(
            inject_output_tool(br#"{"tools":{"external":[{"name":"mine"}]},"model":{}}"#).is_err()
        );
    }

    #[test]
    fn schema_checks_hash_root_and_remote_refs() {
        let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}}});
        let hash = canonical_hash(&schema).unwrap();
        assert!(validate_schema(&schema, &hash).is_ok());
        assert!(validate_schema(&schema, &"0".repeat(64)).is_err());
        let remote =
            json!({"type":"object","properties":{"x":{"$ref":"https://example.test/schema"}}});
        let remote_hash = canonical_hash(&remote).unwrap();
        assert!(validate_schema(&remote, &remote_hash).is_err());
    }

    #[tokio::test]
    async fn executor_repairs_validates_and_replays_exact_decisions() {
        let db = db_with_session(true).await;
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "integer"}},
            "required": ["answer"],
            "additionalProperties": false
        });
        let schema_hash = canonical_hash(&schema).unwrap();
        let body = serde_json::to_vec(&json!({
            "content": "Answer with an integer.",
            "output": {"schema": schema, "schema_hash": schema_hash, "retries": 1}
        }))
        .unwrap();
        let (output_id, transformed) =
            match prepare_message(&db, SESSION_ID, true, &body, Some("message-key"))
                .await
                .unwrap()
            {
                PreparedMessage::New {
                    body, output_id, ..
                } => (output_id, serde_json::from_slice::<Value>(&body).unwrap()),
                _ => panic!("expected a new output request"),
            };
        assert!(transformed.get("output").is_none());
        assert_eq!(
            transformed["metadata"][OUTPUT_CONTEXT_KEY],
            Value::String(output_id.clone())
        );
        assert!(
            transformed["content"]
                .as_str()
                .unwrap()
                .contains("JSON Schema")
        );

        let request = |call_id: &str, input: Value| {
            serde_json::from_value::<ExternalToolCallRequest>(json!({
                "session_id": SESSION_ID,
                "turn_id": "trn_12345678901234567890",
                "agent_id": "root",
                "call_id": call_id,
                "name": OUTPUT_TOOL_NAME,
                "input": input,
                "context": {(OUTPUT_CONTEXT_KEY): output_id.clone()}
            }))
            .unwrap()
        };
        let invalid_request = request("op_invalid", json!({"answer": "wrong"}));
        let invalid = execute(&db, invalid_request.clone()).await.unwrap();
        assert_eq!(invalid["disposition"], "continue");
        assert_eq!(invalid, execute(&db, invalid_request).await.unwrap());
        assert_eq!(
            db.output_request(output_id.clone())
                .await
                .unwrap()
                .unwrap()
                .attempts,
            1,
            "same call id must not consume another attempt"
        );

        let completed = execute(&db, request("op_valid", json!({"answer": 42})))
            .await
            .unwrap();
        assert_eq!(completed["disposition"], "complete_turn");
        assert_eq!(completed["result"], json!({"answer": 42}));
        let row = db.output_request(output_id).await.unwrap().unwrap();
        assert_eq!(row.status, "completed");
        assert_eq!(row.attempts, 2);
    }

    #[tokio::test]
    async fn old_sessions_are_rejected_before_output_admission() {
        let db = db_with_session(false).await;
        let schema = json!({"type": "object"});
        let body = serde_json::to_vec(&json!({
            "content": "answer",
            "output": {"schema": schema, "schema_hash": canonical_hash(&schema).unwrap()}
        }))
        .unwrap();
        let error = prepare_message(&db, SESSION_ID, false, &body, Some("old-session"))
            .await
            .err()
            .expect("old session must fail");
        assert!(matches!(error, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn zero_retries_fails_the_turn_on_the_first_invalid_candidate() {
        let db = db_with_session(true).await;
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "integer"}},
            "required": ["answer"],
            "additionalProperties": false
        });
        let body = serde_json::to_vec(&json!({
            "content": "Answer with an integer.",
            "output": {
                "schema": schema,
                "schema_hash": canonical_hash(&schema).unwrap(),
                "retries": 0
            }
        }))
        .unwrap();
        let output_id = match prepare_message(&db, SESSION_ID, true, &body, Some("no-repair"))
            .await
            .unwrap()
        {
            PreparedMessage::New { output_id, .. } => output_id,
            _ => panic!("expected a new output request"),
        };
        let request = serde_json::from_value::<ExternalToolCallRequest>(json!({
            "session_id": SESSION_ID,
            "turn_id": "trn_12345678901234567890",
            "agent_id": "root",
            "call_id": "op_first_invalid",
            "name": OUTPUT_TOOL_NAME,
            "input": {"answer": "wrong"},
            "context": {(OUTPUT_CONTEXT_KEY): output_id.clone()}
        }))
        .unwrap();

        let failed = execute(&db, request.clone()).await.unwrap();
        assert_eq!(failed["disposition"], "fail_turn");
        assert_eq!(failed["error"]["code"], "output_validation_error");
        assert!(failed["error"]["details"]["issues"].is_array());
        assert_eq!(failed, execute(&db, request).await.unwrap());
        let row = db.output_request(output_id).await.unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert_eq!(row.attempts, 1, "replay must not consume another attempt");
    }

    #[tokio::test]
    async fn an_unadmitted_output_identity_can_be_abandoned() {
        let db = db_with_session(true).await;
        let schema = json!({"type": "object"});
        let body = serde_json::to_vec(&json!({
            "content": "answer",
            "output": {
                "schema": schema,
                "schema_hash": canonical_hash(&schema).unwrap()
            }
        }))
        .unwrap();
        let output_id = match prepare_message(&db, SESSION_ID, true, &body, None)
            .await
            .unwrap()
        {
            PreparedMessage::New { output_id, .. } => output_id,
            _ => panic!("expected a new output request"),
        };
        assert!(
            db.output_request(output_id.clone())
                .await
                .unwrap()
                .is_some()
        );

        db.abandon_output(output_id.clone()).await.unwrap();
        assert!(db.output_request(output_id).await.unwrap().is_none());
    }
}
