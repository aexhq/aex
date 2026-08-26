//! Per-message typed output orchestration owned by the trusted control plane.
//!
//! The model sees one stable, create-time tool and a normal tail instruction containing the
//! per-message schema. Candidate validation runs here, never in the customer's hand. Brain only
//! knows the generic external-tool and return-direct contracts.

use brain_protocol::session::{ExternalToolCallRequest, ExternalToolCallResponse};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::identity;
use crate::store::{BeginOutput, Db, OutputRequestRow};
use crate::web::{FETCH_CAPABILITY, FETCH_TOOL_NAME, SEARCH_CAPABILITY, SEARCH_TOOL_NAME};
use crate::{Error, Result, now_ms};

pub const OUTPUT_TOOL_NAME: &str = "aex_submit_output";
pub const OUTPUT_CAPABILITY: &str = "aex.output";
pub const OUTPUT_CONTEXT_KEY: &str = "aex.output_request_id";
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 4096;
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

/// Seal the host's half of a hosted create: resolve the provider the caller named into the model
/// configuration Brain takes, and append Aex's stable output capability to Brain's native ordered
/// Tool grant. Existing native Tool values remain unchanged and in the same order; the Aex
/// capability is always last. Returns the provider name for the session's read view.
pub fn seal_create_request(body: impl AsRef<[u8]>) -> Result<(Vec<u8>, String)> {
    let mut document: Value = serde_json::from_slice(body.as_ref())
        .map_err(|error| Error::Invalid(format!("session body: {error}")))?;
    // The hosted create path passes its bounded Bytes by value. Release that 24 MiB allocation
    // before serializing the amended document so the normal case holds input+tree or tree+output,
    // not all three simultaneously. Slice-based unit callers remain allocation-free here.
    drop(body);
    let root = document
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("session body must be an object".into()))?;
    match root.remove("shape") {
        None => {}
        Some(Value::String(shape)) if shape == "1gb" => {}
        Some(Value::String(_)) => {
            return Err(Error::Unprocessable(
                "hosted alpha supports only shape `1gb` (0.5 vCPU and 1 GiB)".into(),
            ));
        }
        Some(_) => return Err(Error::Invalid("shape must be a string".into())),
    }
    let provider = crate::model::seal_model(root)?;
    let tools = root.entry("tools").or_insert_with(|| json!({}));
    let tools = tools
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("tools must be an object".into()))?;
    if tools.remove("external").is_some() {
        return Err(Error::Invalid(
            "legacy tools.external is not accepted by this Aex revision".into(),
        ));
    }
    if tools.remove("builtin").is_some() {
        return Err(Error::Invalid(
            "legacy tools.builtin is not accepted by this Aex revision; use native Tool values"
                .into(),
        ));
    }
    let items = tools.entry("items").or_insert_with(|| json!([]));
    let items = items
        .as_array_mut()
        .ok_or_else(|| Error::Invalid("tools.items must be an array".into()))?;
    for item in items.iter_mut() {
        normalize_managed_tool(item)?;
    }
    let output_definition = definition_with_digest(json!({
        "name": OUTPUT_TOOL_NAME,
        "description": "Submit the final structured result requested by the latest user message. Call this once with exactly the result object; do not wrap it in another property.",
        "input_schema": {
            "type": "object",
            "properties": {},
            "additionalProperties": true
        },
        "output_schema": {
            "$comment": "The trusted Aex executor returns the validated structured result."
        }
    }))?;
    items.push(json!({
        "definition": output_definition,
        "executor": {
            "kind": "engine",
            "capability": OUTPUT_CAPABILITY
        }
    }));
    let body = serde_json::to_vec(&document)
        .map_err(|error| Error::Internal(format!("session body: {error}")))?;
    Ok((body, provider))
}

fn normalize_managed_tool(item: &mut Value) -> Result<()> {
    let name = item.pointer("/definition/name").and_then(Value::as_str);
    let capability = item.pointer("/executor/capability").and_then(Value::as_str);
    for (managed_name, managed_capability) in [
        (SEARCH_TOOL_NAME, SEARCH_CAPABILITY),
        (FETCH_TOOL_NAME, FETCH_CAPABILITY),
    ] {
        if name == Some(managed_name) || capability == Some(managed_capability) {
            if name != Some(managed_name)
                || capability != Some(managed_capability)
                || item.pointer("/executor/kind").and_then(Value::as_str) != Some("engine")
            {
                return Err(Error::Invalid(format!(
                    "Aex-managed Tool {managed_name} must use its pinned name and capability"
                )));
            }
            *item = pinned_official_tool(managed_name)?;
            return Ok(());
        }
    }
    if name.is_some_and(|name| {
        name.starts_with("aex_") || matches!(name, SEARCH_TOOL_NAME | FETCH_TOOL_NAME)
    }) || capability.is_some_and(|capability| capability.starts_with("aex."))
    {
        return Err(Error::Invalid(
            "Aex-managed Tool names and aex.* server capabilities are reserved".into(),
        ));
    }
    Ok(())
}

fn pinned_official_tool(name: &str) -> Result<Value> {
    managed_web_tool(name)
}

fn managed_web_tool(name: &str) -> Result<Value> {
    let (definition, capability) = match name {
        SEARCH_TOOL_NAME => (
            json!({
                "name": SEARCH_TOOL_NAME,
                "description": "Search the public web using Aex's managed search service.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "minLength": 1, "maxLength": 500},
                        "num": {"type": "integer", "minimum": 1, "maximum": 10, "default": 5},
                        "country": {"type": "string", "minLength": 2, "maxLength": 8},
                        "language": {"type": "string", "minLength": 2, "maxLength": 16}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                },
                "output_schema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "results": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": {"type": "string"},
                                    "url": {"type": "string"},
                                    "snippet": {"type": "string"},
                                    "date": {"type": "string"}
                                },
                                "required": ["title", "url", "snippet"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["query", "results"],
                    "additionalProperties": false
                }
            }),
            SEARCH_CAPABILITY,
        ),
        FETCH_TOOL_NAME => (
            json!({
                "name": FETCH_TOOL_NAME,
                "description": "Fetch a public text, HTML, or JSON URL through Aex's guarded network service.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "format": "uri"},
                        "max_chars": {"type": "integer", "minimum": 1, "maximum": 100000}
                    },
                    "required": ["url"],
                    "additionalProperties": false
                },
                "output_schema": {
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"},
                        "status": {"type": "integer"},
                        "content_type": {"type": "string"},
                        "text": {"type": "string"},
                        "truncated": {"type": "boolean"}
                    },
                    "required": ["url", "status", "content_type", "text", "truncated"],
                    "additionalProperties": false
                }
            }),
            FETCH_CAPABILITY,
        ),
        _ => unreachable!("managed web Tool name was checked"),
    };
    Ok(json!({
        "definition": definition_with_digest(definition)?,
        "executor": {"kind": "engine", "capability": capability}
    }))
}

fn definition_with_digest(mut definition: Value) -> Result<Value> {
    let digest = canonical_hash(&definition)?;
    definition
        .as_object_mut()
        .ok_or_else(|| Error::Internal("trusted Tool definition is not an object".into()))?
        .insert("contract_digest".into(), Value::String(digest));
    Ok(definition)
}

pub async fn prepare_message(
    db: &Db,
    session_id: &str,
    body: &[u8],
    idempotency_key: Option<&str>,
) -> Result<PreparedMessage> {
    let mut document: Value = serde_json::from_slice(body)
        .map_err(|error| Error::Invalid(format!("message body: {error}")))?;
    let request_hash = canonical_hash(&document)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("message body must be an object".into()))?;
    reject_reserved_metadata(root)?;
    let Some(output) = root.remove("output") else {
        return Ok(PreparedMessage::Plain(body.to_vec()));
    };
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
    // Absent means the default; present-but-not-an-integer is invalid, never the default.
    let retries = match output.get("retries") {
        None => 1,
        Some(value) => value
            .as_i64()
            .filter(|retries| (0..=2).contains(retries))
            .ok_or_else(|| Error::Invalid("output.retries must be 0, 1, or 2".into()))?,
    };
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
    let body = encode_prepared_message(&document)?;
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
        BeginOutput::Existing(row) => match row.accepted_json {
            Some(accepted_json) => return Ok(PreparedMessage::Replay { accepted_json }),
            None => {
                // The earlier control-to-Brain request may have committed even when its HTTP
                // response was lost. Reuse both durable identities and reconstruct the exact
                // transformed bytes; Brain's message idempotency key can then return the original
                // acceptance instead of seeing a different output request as a key conflict.
                let metadata = document
                    .get_mut("metadata")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| {
                        Error::Internal("prepared message metadata is missing".into())
                    })?;
                metadata.insert(OUTPUT_CONTEXT_KEY.into(), Value::String(row.id.clone()));
                let body = encode_prepared_message(&document)?;
                return Ok(PreparedMessage::New {
                    body,
                    output_id: row.id,
                    schema_hash: row.schema_hash,
                });
            }
        },
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

fn encode_prepared_message(document: &Value) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(document)
        .map_err(|error| Error::Internal(format!("message body: {error}")))?;
    if body.len() > brain_protocol::MAX_MESSAGE_REQUEST_BYTES {
        return Err(Error::PayloadTooLarge(format!(
            "structured-output message expands beyond the {}-byte journal ceiling",
            brain_protocol::MAX_MESSAGE_REQUEST_BYTES
        )));
    }
    Ok(body)
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
    if request.context.get("brain.capability").map(String::as_str) != Some(OUTPUT_CAPABILITY) {
        return Err(Error::Invalid(
            "the sealed Aex output capability is missing from the executor call".into(),
        ));
    }
    if request.agent_id.to_string() != "root" {
        return Err(Error::Forbidden(
            "only the root Brain agent may submit Aex structured output".into(),
        ));
    }
    let Some(output_id) = request.context.get(OUTPUT_CONTEXT_KEY).cloned() else {
        return Ok(unarmed_response());
    };
    let Some(row) = db
        .output_request(output_id.clone())
        .await?
        .filter(|row| row.session_id == session_id)
    else {
        return Ok(unarmed_response());
    };
    if !db
        .claim_output_turn(
            row.id.clone(),
            session_id.clone(),
            request.turn_id.to_string(),
        )
        .await?
    {
        return Err(Error::Forbidden(
            "structured output is armed for a different Brain turn".into(),
        ));
    }
    if let Some(response) = db
        .external_call_response(session_id.clone(), call_id.clone())
        .await?
    {
        return serde_json::from_str(&response)
            .map_err(|error| Error::Internal(format!("cached executor response: {error}")));
    }
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
    let terminal_bytes = completed_terminal_bytes(&request.input, &row.id, &row.schema_hash)?;
    let completed_response = json!({
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
    let mut issues = if !external_response_fits(&completed_response)? {
        vec![json!({
            "path": "",
            "message": format!(
                "result terminal projection is {terminal_bytes} bytes or another canonical response projection exceeds the {}-byte maximum",
                brain_protocol::MAX_TOOL_TERMINAL_INLINE_BYTES
            ),
            "keyword": "maxBytes"
        })]
    } else {
        validation_issues(&schema, &request.input)?
    };
    issues.truncate(MAX_ISSUES);
    for issue in &mut issues {
        bound_validation_issue(issue);
    }
    let attempt = row.attempts + 1;
    let (response, status, result_json, error_json) =
        if issues.is_empty() {
            (
                completed_response,
                "completed".to_string(),
                Some(String::from_utf8(candidate_bytes).map_err(|error| {
                    Error::Internal(format!("candidate canonical JSON: {error}"))
                })?),
                None,
            )
        } else {
            let (response, status, error_json) =
                bounded_validation_response(&mut issues, attempt, row.max_attempts)?;
            (response, status, None, Some(error_json))
        };
    if !external_response_fits(&response)? {
        return Err(Error::Internal(
            "structured output response exceeds Brain's canonical inline or transport bound".into(),
        ));
    }
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
            row.attempts,
            now_ms(),
        )
        .await?;
    serde_json::from_str(&stored)
        .map_err(|error| Error::Internal(format!("executor response: {error}")))
}

/// Return-direct output is retained as a terminal projection, not just the bare model value.
/// Measure the exact canonical Brain representation so metadata cannot push a nominally valid
/// result over the shared terminal-inline ceiling after its effect has been accepted.
fn completed_terminal_bytes(result: &Value, output_id: &str, schema_hash: &str) -> Result<usize> {
    brain_protocol::contract::terminal_inline_bytes(&json!({
        "value": result,
        "metadata": {
            "output_id": output_id,
            "schema_hash": schema_hash
        }
    }))
    .map_err(|error| Error::Internal(format!("output terminal projection: {error}")))
}

fn external_response_fits(response: &Value) -> Result<bool> {
    let Ok(response) = serde_json::from_value::<ExternalToolCallResponse>(response.clone()) else {
        return Ok(false);
    };
    Ok(
        brain_protocol::contract::external_tool_response_inline_fits(&response)
            && brain_protocol::contract::external_tool_response_wire_fits(&response),
    )
}

/// A validation failure is model-visible recovery information, so it must itself fit the exact
/// Brain response bounds. Pathological property names and values can make validator diagnostics
/// much larger than the submitted value. Bound individual fields, then retain the longest prefix
/// of issues whose complete retry/fail response is valid.
fn bounded_validation_response(
    issues: &mut Vec<Value>,
    attempt: i64,
    max_attempts: i64,
) -> Result<(Value, String, String)> {
    loop {
        let retry = attempt < max_attempts;
        let error = json!({
            "code": "output_validation_error",
            "message": format!(
                "Model output did not satisfy the schema after {attempt} attempt(s)"
            ),
            "details": {"issues": issues}
        });
        let response = if retry {
            json!({
                "outcome": "failed",
                "content": format!(
                    "The submitted object did not match the requested schema. Correct it and call `{OUTPUT_TOOL_NAME}` again. Validation issues: {}",
                    serde_json::to_string(issues).unwrap_or_else(|_| "[]".into())
                ),
                "is_error": true,
                "disposition": "continue"
            })
        } else {
            json!({
                "outcome": "failed",
                "content": "Structured output validation failed and the configured attempt limit was reached.",
                "is_error": true,
                "disposition": "fail_turn",
                "error": error
            })
        };
        if external_response_fits(&response)? {
            let stored = if retry {
                json!({"issues": issues}).to_string()
            } else {
                error.to_string()
            };
            return Ok((
                response,
                if retry { "pending" } else { "failed" }.into(),
                stored,
            ));
        }
        if issues.pop().is_none() {
            return Err(Error::Internal(
                "the minimal structured-output validation response exceeds Brain's bounds".into(),
            ));
        }
    }
}

fn bound_validation_issue(issue: &mut Value) {
    let Some(issue) = issue.as_object_mut() else {
        *issue = json!({"path":"", "message":"Validation failed", "keyword":"invalid"});
        return;
    };
    for (name, maximum) in [("path", 2_048), ("message", 4_096), ("keyword", 128)] {
        if let Some(value) = issue.get_mut(name)
            && let Some(text) = value.as_str()
        {
            *value = Value::String(prefix_bytes(text, maximum));
        }
    }
}

fn prefix_bytes(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = 0;
    for (offset, character) in value.char_indices() {
        let next = offset + character.len_utf8();
        if next > maximum {
            break;
        }
        end = next;
    }
    value[..end].to_owned()
}

fn unarmed_response() -> Value {
    json!({
        "outcome": "failed",
        "content": "No structured output is requested for this turn. Continue the task normally and do not call this Tool again unless a later user message requests structured output.",
        "is_error": true,
        "disposition": "continue"
    })
}

fn validate_schema(schema: &Value, claimed_hash: &str) -> Result<String> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::OutputSchema(
            "output.schema must have an object root".into(),
        ));
    }
    let canonical = serde_jcs::to_vec(schema)
        .map_err(|error| Error::OutputSchema(format!("output.schema: {error}")))?;
    if canonical.len() > MAX_SCHEMA_BYTES {
        return Err(Error::OutputSchema(format!(
            "output.schema is {} bytes; maximum is {MAX_SCHEMA_BYTES}",
            canonical.len()
        )));
    }
    let actual = hex::encode(Sha256::digest(&canonical));
    if actual != claimed_hash {
        return Err(Error::OutputSchema(format!(
            "output.schema_hash mismatch; expected {actual}"
        )));
    }
    let mut nodes = 0;
    inspect_schema(schema, 0, &mut nodes)?;
    jsonschema::meta::validate(schema)
        .map_err(|error| Error::OutputSchema(format!("output.schema: {error}")))?;
    jsonschema::draft202012::new(schema)
        .map_err(|error| Error::OutputSchema(format!("output.schema: {error}")))?;
    String::from_utf8(canonical)
        .map_err(|error| Error::Internal(format!("canonical output schema: {error}")))
}

fn reject_reserved_metadata(root: &serde_json::Map<String, Value>) -> Result<()> {
    if root
        .get("metadata")
        .and_then(Value::as_object)
        .is_some_and(|metadata| metadata.keys().any(|key| key.starts_with("aex.")))
    {
        return Err(Error::Invalid(
            "message metadata keys beginning with aex. are reserved".into(),
        ));
    }
    Ok(())
}

fn inspect_schema(value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(Error::OutputSchema(format!(
            "output.schema exceeds depth {MAX_SCHEMA_DEPTH}"
        )));
    }
    *nodes += 1;
    if *nodes > MAX_SCHEMA_NODES {
        return Err(Error::OutputSchema(format!(
            "output.schema exceeds {MAX_SCHEMA_NODES} nodes"
        )));
    }
    match value {
        Value::Object(object) => {
            for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
                if let Some(reference) = object.get(keyword).and_then(Value::as_str)
                    && !reference.starts_with('#')
                {
                    return Err(Error::OutputSchema(format!(
                        "output.schema remote {keyword} values are not supported"
                    )));
                }
            }
            if object.contains_key("pattern") || object.contains_key("patternProperties") {
                return Err(Error::OutputSchema(
                    "output.schema regular-expression keywords are not supported in the MVP".into(),
                ));
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

    #[test]
    fn output_reserves_exact_terminal_projection_headroom() {
        let output_id = "out_12345678901234567890";
        let schema_hash = &"a".repeat(64);
        let empty = json!({"data": ""});
        let envelope_bytes = completed_terminal_bytes(&empty, output_id, schema_hash).unwrap();
        let limit = brain_protocol::MAX_TOOL_TERMINAL_INLINE_BYTES;
        assert!(envelope_bytes < limit);

        let exact = json!({"data": "x".repeat(limit - envelope_bytes)});
        assert_eq!(
            completed_terminal_bytes(&exact, output_id, schema_hash).unwrap(),
            limit
        );
        assert!(
            external_response_fits(&json!({
                "outcome": "completed",
                "content": "Structured output accepted.",
                "is_error": false,
                "disposition": "complete_turn",
                "result": exact,
                "result_metadata": {"output_id": output_id, "schema_hash": schema_hash}
            }))
            .unwrap()
        );
        let over = json!({"data": "x".repeat(limit - envelope_bytes + 1)});
        assert_eq!(
            completed_terminal_bytes(&over, output_id, schema_hash).unwrap(),
            limit + 1
        );
        assert!(
            !external_response_fits(&json!({
                "outcome": "completed",
                "content": "Structured output accepted.",
                "is_error": false,
                "disposition": "complete_turn",
                "result": over,
                "result_metadata": {"output_id": output_id, "schema_hash": schema_hash}
            }))
            .unwrap()
        );
    }

    #[test]
    fn pathological_validation_diagnostics_remain_normal_tool_failures() {
        for (attempt, maximum, disposition) in [(1, 2, "continue"), (2, 2, "fail_turn")] {
            let mut issues = (0..MAX_ISSUES)
                .map(|_| {
                    json!({
                        "path": format!("/{}", "p".repeat(10_000)),
                        "message": "\u{0001}".repeat(10_000),
                        "keyword": "k".repeat(10_000)
                    })
                })
                .collect::<Vec<_>>();
            for issue in &mut issues {
                bound_validation_issue(issue);
            }
            let original = issues.len();
            let (response, status, stored) =
                bounded_validation_response(&mut issues, attempt, maximum).unwrap();
            assert!(external_response_fits(&response).unwrap());
            assert_eq!(response["disposition"], disposition);
            assert_eq!(
                status,
                if attempt < maximum {
                    "pending"
                } else {
                    "failed"
                }
            );
            assert!(!stored.is_empty());
            assert!(
                issues.len() < original,
                "escaped diagnostics must be trimmed"
            );
            assert!(issues.first().is_some_and(|issue| {
                issue["path"].as_str().unwrap().len() <= 2_048
                    && issue["message"].as_str().unwrap().len() <= 4_096
                    && issue["keyword"].as_str().unwrap().len() <= 128
            }));
        }
    }

    async fn db_with_session() -> Db {
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
            provider: "openai".into(),
            id: SESSION_ID.into(),
            account_id: "acc_output".into(),
            key_id: "key_output".into(),
            parent_id: None,
            root_id: SESSION_ID.into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: crate::rating::FoldState::default(),
        })
        .await
        .unwrap();
        db
    }

    /// Every hosted create resolves its provider before the Tool grant is sealed.
    fn seal(body: &[u8]) -> Result<Vec<u8>> {
        seal_create_request(body).map(|(body, _)| body)
    }

    #[test]
    fn injection_appends_the_native_output_tool_without_mutating_ordinary_tools() {
        let injected = seal(
            br#"{
                "model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},
                "tools":{"items":[{
                    "definition":{
                        "name":"delegate",
                        "description":"Delegate work.",
                        "input_schema":{"type":"object"},
                        "output_schema":{"type":"object"}
                    },
                    "executor":{"kind":"environment","environment":"application","callback_registration":"delegate","requirements":{}}
                }]}
            }"#,
        )
        .unwrap();
        let document: Value = serde_json::from_slice(&injected).unwrap();
        assert!(document.get("shape").is_none());
        assert_eq!(document["tools"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            document["tools"]["items"][0]["definition"]["name"],
            "delegate"
        );
        assert_eq!(
            document["tools"]["items"][1]["definition"]["name"],
            OUTPUT_TOOL_NAME
        );
        assert_eq!(
            document["tools"]["items"][1]["executor"]["capability"],
            OUTPUT_CAPABILITY
        );
        assert!(document["tools"].get("external").is_none());
        assert!(
            seal(br#"{"tools":{"external":[{"name":"mine"}]},"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).is_err()
        );
        assert!(seal(br#"{"tools":{"external":[]},"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).is_err());
        assert!(seal(br#"{"tools":{"builtin":["bash"]},"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).is_err());
        assert!(seal(br#"{"tools":{"builtin":[]},"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).is_err());
        let hosted_shape = seal(br#"{"shape":"1gb","model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).unwrap();
        let hosted_shape: Value = serde_json::from_slice(&hosted_shape).unwrap();
        assert!(hosted_shape.get("shape").is_none());
        let unsupported = seal(br#"{"shape":"2gb","model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).unwrap_err();
        assert!(matches!(unsupported, Error::Unprocessable(_)));
        assert_eq!(unsupported.status(), 422);
        assert!(seal(br#"{"shape":2,"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}}"#).is_err());
        assert_eq!(OUTPUT_CAPABILITY, "aex.output");
    }

    #[test]
    fn injection_pins_managed_web_tools_and_rejects_namespace_spoofing() {
        assert_eq!(SEARCH_CAPABILITY, "aex.web.search");
        assert_eq!(FETCH_CAPABILITY, "aex.web.fetch");
        let injected = seal(
            br#"{
                "model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},
                "tools":{"items":[{
                    "definition":{
                        "name":"web_search",
                        "description":"attacker controlled",
                        "input_schema":{"type":"string"},
                        "output_schema":{"type":"string"}
                    },
                    "executor":{
                        "kind":"engine",
                        "capability":"aex.web.search"
                    }
                }]}
            }"#,
        )
        .unwrap();
        let document: Value = serde_json::from_slice(&injected).unwrap();
        let search = &document["tools"]["items"][0];
        assert_eq!(
            search["definition"]["description"],
            "Search the public web using Aex's managed search service."
        );
        assert_eq!(
            search["executor"],
            json!({"kind": "engine", "capability": SEARCH_CAPABILITY})
        );

        assert!(
            seal(
                br#"{"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},"tools":{"items":[{"definition":{"name":"other","description":"x","input_schema":{},"output_schema":{}},"executor":{"kind":"engine","capability":"aex.web.search"}}]}}"#
            )
            .is_err()
        );
        assert!(
            seal(
                br#"{"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},"tools":{"items":[{"definition":{"name":"web_fetch","description":"x","input_schema":{},"output_schema":{}},"executor":{"kind":"aex_managed","bundle_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"preinstalled","required_env":[]}}]}}"#
            )
            .is_err()
        );
        assert!(
            seal(
                br#"{"model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},"tools":{"items":[{"definition":{"name":"other","description":"x","input_schema":{},"output_schema":{}},"executor":{"kind":"engine","capability":"aex.private"}}]}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn injection_passes_component_subagents_through_verbatim() {
        let source = br#"{
                "model":{"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"},
                "tools":{"items":[{
                    "definition":{
                        "name":"subagents",
                        "description":"Create child sessions.",
                        "input_schema":{"type":"object"},
                        "output_schema":{},
                        "contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    },
                    "executor":{
                        "kind":"component",
                        "component_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "world":"aex:tool/tool@1.0.0",
                        "config":{"definition":{"name":"subagents"}},
                        "grants":["children"]
                    }
                }]}
            }"#;
        let source_document: Value = serde_json::from_slice(source).unwrap();
        let injected = seal(source).unwrap();
        let document: Value = serde_json::from_slice(&injected).unwrap();
        let subagents = &document["tools"]["items"][0];
        assert_eq!(subagents, &source_document["tools"]["items"][0]);
        assert_eq!(subagents["executor"]["grants"], json!(["children"]));
        assert!(document.get("secrets").is_none());
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
        let pattern =
            json!({"type":"object","properties":{"x":{"type":"string","pattern":"^(a+)+$"}}});
        let pattern_hash = canonical_hash(&pattern).unwrap();
        assert!(validate_schema(&pattern, &pattern_hash).is_err());
    }

    #[tokio::test]
    async fn reserved_metadata_is_rejected_even_without_an_output_request() {
        let db = db_with_session().await;
        let body = serde_json::to_vec(&json!({
            "content": "plain message",
            "metadata": {OUTPUT_CONTEXT_KEY: "out_attacker_controlled"}
        }))
        .unwrap();
        assert!(matches!(
            prepare_message(&db, SESSION_ID, &body, Some("reserved-metadata")).await,
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn executor_repairs_validates_and_replays_exact_decisions() {
        let db = db_with_session().await;
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
            match prepare_message(&db, SESSION_ID, &body, Some("message-key"))
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
                "context": {
                    "brain.capability": OUTPUT_CAPABILITY,
                    (OUTPUT_CONTEXT_KEY): output_id.clone()
                }
            }))
            .unwrap()
        };
        let mut unsealed = request("op_unsealed", json!({"answer": 42}));
        unsealed.context.remove("brain.capability");
        assert!(matches!(
            execute(&db, unsealed).await,
            Err(Error::Invalid(_))
        ));
        let mut unarmed = request("op_unarmed", json!({"answer": 42}));
        unarmed.context.remove(OUTPUT_CONTEXT_KEY);
        let unarmed = execute(&db, unarmed).await.unwrap();
        assert_eq!(unarmed["disposition"], "continue");
        assert_eq!(unarmed["is_error"], true);
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

        let mut wrong_turn =
            serde_json::to_value(request("op_wrong_turn", json!({"answer": 42}))).unwrap();
        wrong_turn["turn_id"] = "trn_00000000000000000000".into();
        assert!(matches!(
            execute(
                &db,
                serde_json::from_value::<ExternalToolCallRequest>(wrong_turn).unwrap()
            )
            .await,
            Err(Error::Forbidden(_))
        ));
        let mut child = serde_json::to_value(request("op_child", json!({"answer": 42}))).unwrap();
        child["agent_id"] = "agent_child".into();
        assert!(matches!(
            execute(
                &db,
                serde_json::from_value::<ExternalToolCallRequest>(child).unwrap()
            )
            .await,
            Err(Error::Forbidden(_))
        ));

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
    async fn zero_retries_fails_the_turn_on_the_first_invalid_candidate() {
        let db = db_with_session().await;
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
        let output_id = match prepare_message(&db, SESSION_ID, &body, Some("no-repair"))
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
            "context": {
                "brain.capability": OUTPUT_CAPABILITY,
                (OUTPUT_CONTEXT_KEY): output_id.clone()
            }
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
        let db = db_with_session().await;
        let schema = json!({"type": "object"});
        let body = serde_json::to_vec(&json!({
            "content": "answer",
            "output": {
                "schema": schema,
                "schema_hash": canonical_hash(&schema).unwrap()
            }
        }))
        .unwrap();
        let output_id = match prepare_message(&db, SESSION_ID, &body, None).await.unwrap() {
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

    #[tokio::test]
    async fn an_ambiguous_admission_retry_reuses_the_durable_output_identity() {
        let db = db_with_session().await;
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "integer"}},
            "required": ["answer"]
        });
        let body = serde_json::to_vec(&json!({
            "content": "answer",
            "output": {
                "schema": schema,
                "schema_hash": canonical_hash(&schema).unwrap()
            }
        }))
        .unwrap();
        let first = prepare_message(&db, SESSION_ID, &body, Some("ambiguous-message"))
            .await
            .unwrap();
        let second = prepare_message(&db, SESSION_ID, &body, Some("ambiguous-message"))
            .await
            .unwrap();
        let (first_body, first_id, first_schema) = match first {
            PreparedMessage::New {
                body,
                output_id,
                schema_hash,
            } => (body, output_id, schema_hash),
            _ => panic!("first preparation must allocate an identity"),
        };
        let (second_body, second_id, second_schema) = match second {
            PreparedMessage::New {
                body,
                output_id,
                schema_hash,
            } => (body, output_id, schema_hash),
            _ => panic!("an unconfirmed retry must be dispatchable"),
        };
        assert_eq!(second_id, first_id);
        assert_eq!(second_schema, first_schema);
        assert_eq!(
            second_body, first_body,
            "Brain must see byte-identical input"
        );

        db.accept_output(
            first_id,
            "trn_12345678901234567890".into(),
            json!({"session_id":SESSION_ID,"turn_id":"trn_12345678901234567890"}).to_string(),
        )
        .await
        .unwrap();
        assert!(matches!(
            prepare_message(&db, SESSION_ID, &body, Some("ambiguous-message"))
                .await
                .unwrap(),
            PreparedMessage::Replay { .. }
        ));
    }

    #[tokio::test]
    async fn structured_output_expansion_cannot_cross_the_journal_ceiling() {
        let db = Db::open_memory().unwrap();
        let schema = json!({"type": "object"});
        let mut document = json!({
            "content": "",
            "output": {
                "schema": schema,
                "schema_hash": canonical_hash(&schema).unwrap()
            }
        });
        let empty_bytes = serde_json::to_vec(&document).unwrap().len();
        document["content"] = "x"
            .repeat(brain_protocol::MAX_MESSAGE_REQUEST_BYTES - empty_bytes)
            .into();
        let body = serde_json::to_vec(&document).unwrap();
        assert_eq!(body.len(), brain_protocol::MAX_MESSAGE_REQUEST_BYTES);

        assert!(matches!(
            prepare_message(&db, SESSION_ID, &body, None).await,
            Err(Error::PayloadTooLarge(_))
        ));
    }
}
