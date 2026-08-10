//! The tool-exec request names no tenant, and cannot be made to.
//!
//! The sibling of `aex-hands-agent/tests/no_guest_billing.rs`, and the same
//! shape of control: a structural assertion that the type could not carry the
//! thing, and a behavioural one that a caller who tries is refused rather than
//! quietly ignored.
//!
//! Why it is worth a test of its own. The executor spends the platform's money
//! on a customer's behalf and resolves *whose* money from the signed envelope
//! alone. The moment the request body carries an organization, a workspace or a
//! principal, a caller can state one — and a field that is present but unread is
//! one refactor away from being read.

use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::assertion::IssuedAssertion;
use aex_internal_contracts::tool_exec::{
    ArgumentsJcs, DeadlineMs, EffectRef, MAX_ARGUMENTS_JCS_BYTES, MAX_TOOL_EXEC_DEADLINE_MS,
    ToolExecError, ToolExecRefusal, ToolExecRequest, ToolExecResponse, ToolResultPart,
};
use aex_wire::ids::{ContentHash, ResourceName};
use base64::Engine as _;

fn request() -> ToolExecRequest {
    ToolExecRequest {
        schema_version: SchemaVersion::V1,
        assertion: IssuedAssertion::new("QUVYQQ").expect("a bounded canonical spelling"),
        tool: ResourceName::parse("web_search").expect("a catalogue tool name"),
        manifest: ContentHash::from_bytes([9; 32]),
        arguments_jcs: ArgumentsJcs::new(br#"{"query":"aex"}"#.to_vec()).expect("inside the bound"),
        effect: EffectRef::new([0xab; 16]),
        attempt: 0,
        deadline_ms: DeadlineMs::new(15_000).expect("inside the bound"),
    }
}

/// The whole point, stated as a key set.
///
/// Written as an exact list rather than a "does not contain" scan so that a
/// field added later fails here and has to be argued for, whatever it is named.
#[test]
fn the_request_carries_exactly_eight_fields_and_none_of_them_is_a_tenant() {
    let document = serde_json::to_value(request()).expect("a request encodes");
    let object = document.as_object().expect("a request is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "argumentsJcs",
            "assertion",
            "attempt",
            "deadlineMs",
            "effect",
            "manifest",
            "schemaVersion",
            "tool",
        ],
        "the tool-exec request grew a field; if it names a tenant the executor's \
         whole identity argument is gone, and if it does not, say so here"
    );
}

/// A caller who states a tenant is refused, not ignored.
#[test]
fn every_tenant_shaped_field_is_refused_by_the_decoder() {
    let base = serde_json::to_value(request()).expect("encodes");
    let object = base.as_object().expect("an object").clone();

    // The exact document, unmodified, must decode — otherwise the negatives
    // below would pass for the wrong reason.
    assert!(serde_json::from_value::<ToolExecRequest>(base).is_ok());

    for name in [
        "principal",
        "principalId",
        "organization",
        "organizationId",
        "workspace",
        "workspaceId",
        "session",
        "sessionId",
        "agent",
        "tenant",
        "accountId",
        "billingAccount",
    ] {
        let mut document = object.clone();
        document.insert(
            name.to_owned(),
            serde_json::Value::String("org_01kyw2qa4ne00r40r40m30e209".to_owned()),
        );
        let outcome =
            serde_json::from_value::<ToolExecRequest>(serde_json::Value::Object(document));
        assert!(
            outcome.is_err(),
            "`{name}` was accepted; `deny_unknown_fields` is what keeps a stated \
             tenant from being one handler edit away from being believed"
        );
    }
}

/// The transport bound is derived from the catalogue's own frame ceiling, and it
/// is enforced before the bytes are materialised rather than after.
#[test]
fn the_argument_document_is_bounded_at_the_catalogue_frame_ceiling() {
    assert_eq!(
        MAX_ARGUMENTS_JCS_BYTES, 65_536,
        "ToolBounds::max_frame_bytes"
    );

    assert_eq!(
        ArgumentsJcs::new(Vec::new()),
        Err(ToolExecError::ArgumentsTooLarge),
        "canonical JSON is never zero bytes, so an empty document is a dropped field"
    );
    assert!(ArgumentsJcs::new(vec![b'{'; MAX_ARGUMENTS_JCS_BYTES]).is_ok());
    assert_eq!(
        ArgumentsJcs::new(vec![b'{'; MAX_ARGUMENTS_JCS_BYTES + 1]),
        Err(ToolExecError::ArgumentsTooLarge)
    );

    let oversized =
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(vec![b'{'; MAX_ARGUMENTS_JCS_BYTES + 1]);
    assert!(
        serde_json::from_value::<ArgumentsJcs>(serde_json::Value::String(oversized)).is_err(),
        "the decoder must refuse an oversized document"
    );

    // One spelling only. A padded or standard-alphabet encoding of the same
    // bytes is a second wire form of one document, and two forms is how a
    // digest over the document stops meaning anything.
    let padded = base64::engine::general_purpose::URL_SAFE.encode(br#"{"query":"a"}"#);
    assert!(padded.ends_with('='));
    assert!(serde_json::from_value::<ArgumentsJcs>(serde_json::Value::String(padded)).is_err());
}

/// A zero deadline is refused rather than read as "no deadline".
#[test]
fn the_deadline_is_bounded_by_the_lifetime_of_the_envelope_that_authorises_it() {
    // 30 000 ms is both the longest `timeout_ms` any managed-internet tool
    // declares and `ASSERTION_MAX_LIFETIME_MS`. The second is the load-bearing
    // one: a call may not outlive its own authorisation.
    assert_eq!(MAX_TOOL_EXEC_DEADLINE_MS, 30_000);
    assert_eq!(DeadlineMs::new(0), Err(ToolExecError::DeadlineOutOfRange));
    assert_eq!(
        DeadlineMs::new(MAX_TOOL_EXEC_DEADLINE_MS + 1),
        Err(ToolExecError::DeadlineOutOfRange)
    );
    assert_eq!(
        DeadlineMs::new(MAX_TOOL_EXEC_DEADLINE_MS)
            .expect("the ceiling itself is admitted")
            .get(),
        MAX_TOOL_EXEC_DEADLINE_MS
    );
    assert!(
        serde_json::from_value::<DeadlineMs>(serde_json::Value::from(0)).is_err(),
        "the decoder enforces the same bound the constructor does"
    );
}

/// The arguments never reach a log line by being printed.
#[test]
fn the_argument_document_does_not_render_itself() {
    let arguments = ArgumentsJcs::new(br#"{"query":"a secret the model was given"}"#.to_vec())
        .expect("inside the bound");
    let rendered = format!("{arguments:?}");
    assert!(!rendered.contains("secret"), "{rendered}");
    assert_eq!(rendered, "ArgumentsJcs(<40 bytes>)");
}

/// The effect reference has exactly one spelling in both directions.
#[test]
fn an_effect_reference_round_trips_as_lowercase_hexadecimal() {
    let effect = EffectRef::new([0x0a, 0xbc, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff]);
    let document = serde_json::to_string(&effect).expect("encodes");
    assert_eq!(document, "\"0abc00000000000000000000000000ff\"");
    assert_eq!(
        serde_json::from_str::<EffectRef>(&document).expect("decodes"),
        effect
    );
    for wrong in [
        "\"0ABC00000000000000000000000000FF\"",
        "\"0abc\"",
        "\"0abc00000000000000000000000000f\"",
        "\"0abc00000000000000000000000000fg\"",
    ] {
        assert!(
            serde_json::from_str::<EffectRef>(wrong).is_err(),
            "{wrong} decoded"
        );
    }
}

/// A refusal says which ceiling, never how much is left of it.
#[test]
fn a_refusal_carries_no_budget_a_hostile_caller_could_search() {
    let refused = ToolExecResponse::Refused {
        schema_version: SchemaVersion::V1,
        reason: ToolExecRefusal::LimitExceeded,
    };
    let document = serde_json::to_string(&refused).expect("encodes");
    assert_eq!(
        document,
        r#"{"outcome":"refused","schemaVersion":1,"reason":"limit_exceeded"}"#
    );
    assert_eq!(
        serde_json::from_str::<ToolExecResponse>(&document).expect("decodes"),
        refused
    );

    let completed = ToolExecResponse::Completed {
        schema_version: SchemaVersion::V1,
        content: vec![ToolResultPart::Text {
            text: "ok".to_owned(),
        }],
        is_error: false,
        duration_ms: 12,
        checksum: ContentHash::from_bytes([1; 32]),
    };
    let document = serde_json::to_string(&completed).expect("encodes");
    assert_eq!(
        serde_json::from_str::<ToolExecResponse>(&document).expect("decodes"),
        completed
    );
}
