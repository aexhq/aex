use std::collections::BTreeMap;

use aex_brain_domain::DispatchProof;
use aex_wire::{CanonicalJson, ids::ResourceName};
use serde_json::json;

use crate::client::{
    AnnotationExclusion, DiscoveredTool, McpHeaderAnnotations, McpHeaders, PROTOCOL_REVISION,
    QualificationError, ResponseBoundError, ResponseBounds, ToolExclusionReason, qualify_discovery,
};
use crate::recovery::{
    DropRecovery, McpTaskId, TaskCapability, TaskState, after_stream_drop, task_transition,
};

#[test]
fn every_post_has_revision_identity_accept_and_argument_mirrors() {
    let schema = json!({
        "type":"object",
        "properties": {
            "region":{"type":"string", "x-mcp-header":"Region"},
            "enabled":{"type":"boolean", "x-mcp-header":"Enabled"},
            "nested":{"type":"object", "properties": {
                "count":{"type":"integer", "minimum":0, "maximum":100,
                         "x-mcp-header":"Count"}
            }}
        }
    });
    let annotations = McpHeaderAnnotations::validate(&schema).expect("valid annotations");
    let arguments = CanonicalJson::from_value(&json!({
        "region":"eu-west-1", "enabled":true, "nested":{"count":7}
    }))
    .expect("canonical fixture");
    let headers =
        McpHeaders::for_call("tools/call", "天气", &arguments, &annotations).expect("headers");
    assert_eq!(headers.get("mcp-protocol-version"), Some(PROTOCOL_REVISION));
    assert_eq!(headers.get("mcp-method"), Some("tools/call"));
    assert_eq!(headers.get("mcp-name"), Some("=?base64?5aSp5rCU?="));
    assert_eq!(
        headers.get("accept"),
        Some("application/json, text/event-stream")
    );
    assert_eq!(headers.get("mcp-param-region"), Some("eu-west-1"));
    assert_eq!(headers.get("mcp-param-enabled"), Some("true"));
    assert_eq!(headers.get("mcp-param-count"), Some("7"));
    for forbidden in ["mcp-session-id", "last-event-id"] {
        assert_eq!(headers.get(forbidden), None);
    }
}

#[test]
fn invalid_header_annotations_exclude_only_the_annotated_tool() {
    let invalid = [
        json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":""}}}),
        json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":"bad name"}}}),
        json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":"bad\rname"}}}),
        json!({"type":"object","properties":{
            "x":{"type":"string","x-mcp-header":"Same"},
            "y":{"type":"boolean","x-mcp-header":"same"}}}),
        json!({"type":"object","properties":{"x":{"type":"number","x-mcp-header":"X"}}}),
        json!({"type":"object","properties":{"x":{"type":"array","items":{"type":"string","x-mcp-header":"X"}}}}),
        json!({"type":"object","oneOf":[{"properties":{"x":{"type":"string","x-mcp-header":"X"}}}]}),
        json!({"type":"object","properties":{"x":{"$ref":"#/$defs/x","x-mcp-header":"X"}}}),
        json!({"type":"object","properties":{"x":{"type":"integer","minimum":-9_007_199_254_740_992_i64,"maximum":0,"x-mcp-header":"X"}}}),
    ];
    for schema in invalid {
        assert_eq!(
            McpHeaderAnnotations::validate(&schema),
            Err(AnnotationExclusion::InvalidMcpHeaderAnnotation)
        );
    }
    assert!(
        McpHeaderAnnotations::validate(&json!({
            "type":"object", "properties":{"ordinary":{"type":"string"}}
        }))
        .is_ok()
    );
}

#[test]
fn task_identity_is_the_only_stream_recovery_cursor() {
    assert_eq!(
        after_stream_drop(
            TaskCapability::Unsupported,
            DispatchProof::ResponseStarted,
            None
        ),
        DropRecovery::OutcomeUnknown
    );
    assert_eq!(
        after_stream_drop(TaskCapability::Advertised, DispatchProof::NotSent, None),
        DropRecovery::RetrySameEffect
    );
    assert_eq!(
        after_stream_drop(
            TaskCapability::Advertised,
            DispatchProof::PossiblySent,
            None
        ),
        DropRecovery::OutcomeUnknown
    );
    let task = McpTaskId::new(
        ResourceName::parse("server").expect("server name"),
        "task-7",
    )
    .expect("task id");
    assert_eq!(
        after_stream_drop(
            TaskCapability::Advertised,
            DispatchProof::ResponseStarted,
            Some(task.clone())
        ),
        DropRecovery::QuerySameTask(task)
    );
}

#[test]
fn task_polling_clamps_without_holding_a_connection() {
    let working = task_transition(TaskState::Working { poll_after_ms: 12 }, false);
    assert_eq!(
        working,
        crate::recovery::TaskTransition::ScheduleWake { after_ms: 1_000 }
    );
    let working = task_transition(
        TaskState::Working {
            poll_after_ms: 90_000,
        },
        false,
    );
    assert_eq!(
        working,
        crate::recovery::TaskTransition::ScheduleWake { after_ms: 30_000 }
    );
    assert_eq!(
        task_transition(TaskState::InputRequired, false),
        crate::recovery::TaskTransition::CancelThenKnownFailure
    );
    assert_eq!(
        task_transition(
            TaskState::Working {
                poll_after_ms: 5_000
            },
            true
        ),
        crate::recovery::TaskTransition::OutcomeUnknownTaskTtlExpired
    );
}

proptest::proptest! {
    #[test]
    fn headers_are_always_derived_from_the_same_argument_object(value in "[A-Za-z0-9 _-]{0,64}") {
        let schema = json!({"type":"object","properties":{
            "value":{"type":"string","x-mcp-header":"Value"}
        }});
        let annotations = McpHeaderAnnotations::validate(&schema).expect("schema");
        let arguments = CanonicalJson::from_value(&json!({"value":value})).expect("arguments");
        let headers = McpHeaders::for_call("tools/call", "tool", &arguments, &annotations)
            .expect("headers");
        let expected = if value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
            && !value.starts_with("=?base64?") {
            value
        } else {
            format!("=?base64?{}?=", base64::Engine::encode(&base64::prelude::BASE64_STANDARD, value))
        };
        proptest::prop_assert_eq!(headers.get("mcp-param-value"), Some(expected.as_str()));
    }
}

#[test]
fn no_legacy_transport_headers_can_be_inserted_by_call_builder() {
    let arguments = CanonicalJson::from_value(&json!({})).expect("arguments");
    let headers = McpHeaders::for_call(
        "tools/call",
        "simple",
        &arguments,
        &McpHeaderAnnotations::empty(),
    )
    .expect("headers");
    let names = headers.iter().collect::<BTreeMap<_, _>>();
    assert!(!names.contains_key("mcp-session-id"));
    assert!(!names.contains_key("last-event-id"));
}

#[test]
fn hostile_stream_frames_fail_closed_without_truncation() {
    let mut bounds = ResponseBounds::new();
    assert_eq!(
        bounds.accept_event(&vec![b'x'; 65_537], false),
        Err(ResponseBoundError::EventTooLarge)
    );
    assert_eq!(
        bounds.accept_event(&[0xff], false),
        Err(ResponseBoundError::InvalidUtf8)
    );
    let mut deep = "0".to_owned();
    for _ in 0..33 {
        deep = format!("[{deep}]");
    }
    assert_eq!(
        bounds.accept_event(deep.as_bytes(), false),
        Err(ResponseBoundError::JsonTooDeep)
    );

    let notification = br#"{"jsonrpc":"2.0","method":"notifications/progress"}"#;
    for _ in 0..256 {
        bounds
            .accept_event(notification, true)
            .expect("notification within bound");
    }
    assert_eq!(
        bounds.accept_event(notification, true),
        Err(ResponseBoundError::TooManyNotifications)
    );

    let mut total = ResponseBounds::new();
    let event = serde_json::to_vec(&"x".repeat(65_000)).expect("event fixture");
    loop {
        if let Err(error) = total.accept_event(&event, false) {
            assert_eq!(error, ResponseBoundError::ResponseTooLarge);
            break;
        }
    }
}

#[test]
fn qualification_is_exact_bounded_and_excludes_bad_tools_individually() {
    let server = ResourceName::parse("server").expect("server name");
    let tool = |name: &str, schema: serde_json::Value| DiscoveredTool {
        name: name.to_owned(),
        input_schema: schema,
        output_schema: None,
    };
    assert_eq!(
        qualify_discovery(&server, &["2025-11-25".to_owned()], 1, Vec::new()),
        Err(QualificationError::ProtocolVersionUnsupported)
    );
    assert_eq!(
        qualify_discovery(
            &server,
            &[PROTOCOL_REVISION.to_owned()],
            1,
            vec![
                tool("same", json!({"type":"object"})),
                tool("same", json!({"type":"object"}))
            ]
        ),
        Err(QualificationError::DuplicateRemoteName)
    );
    let manifest = qualify_discovery(
        &server,
        &[PROTOCOL_REVISION.to_owned()],
        1,
        vec![
            tool("good", json!({"type":"object"})),
            tool(
                "bad_header",
                json!({"type":"object","properties":{"x":{"type":"number","x-mcp-header":"X"}}}),
            ),
            tool(&"x".repeat(64), json!({"type":"object"})),
        ],
    )
    .expect("bounded discovery");
    assert_eq!(manifest.protocol_revision, PROTOCOL_REVISION);
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(
        manifest.tools[0].advertised_name.as_str(),
        "mcp__server__good"
    );
    assert_eq!(manifest.excluded.len(), 2);
    assert_eq!(
        manifest.excluded[0].reason,
        ToolExclusionReason::InvalidMcpHeaderAnnotation
    );
    assert_eq!(
        manifest.excluded[1].reason,
        ToolExclusionReason::NameInvalid
    );
}

#[test]
fn task_ids_are_bounded_before_becoming_recovery_handles() {
    let server = ResourceName::parse("server").expect("server name");
    assert!(McpTaskId::new(server.clone(), "").is_err());
    assert!(McpTaskId::new(server.clone(), &"x".repeat(257)).is_err());
    assert!(McpTaskId::new(server, "bad\nid").is_err());
}
