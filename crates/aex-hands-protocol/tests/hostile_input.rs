//! Hostile typed-JSON and domain-boundary cases for the guest protocol.

use aex_hands_protocol::lifecycle::{
    KeepaliveLease, ReceiptError, RuntimeReceipt, TrueIdleEvidence,
};
use aex_hands_protocol::operation::{GuestPath, GuestPathError, GuestRoot, SearchPattern};
use aex_hands_protocol::rpc::{
    Fence, GenerationBinding, GuestRequest, HandsOperationId, PROTOCOL_V1, StatusRequest,
};
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
use aex_wire::types::{ComputeSize, Timestamp};

fn generation() -> GenerationId {
    GenerationId::parse("gen_01kyw2qa4ne00r40r40m30e209").expect("generation id")
}

fn request() -> GuestRequest<StatusRequest> {
    GuestRequest {
        binding: GenerationBinding {
            schema_version: PROTOCOL_V1,
            generation: generation(),
            fence: Fence(7),
        },
        request: StatusRequest {
            operation: HandsOperationId(Uuid7::compose(3, [3; 10])),
        },
    }
}

#[test]
fn a_well_formed_typed_envelope_decodes() {
    let encoded = serde_json::to_vec(&request()).expect("serialize");
    let decoded: GuestRequest<StatusRequest> =
        serde_json::from_slice(&encoded).expect("a valid envelope decodes");
    assert_eq!(decoded, request());
}

#[test]
fn every_truncation_and_non_object_shape_is_refused_without_a_panic() {
    let encoded = serde_json::to_vec(&request()).expect("serialize");
    for cut in 0..encoded.len() {
        assert!(
            serde_json::from_slice::<GuestRequest<StatusRequest>>(&encoded[..cut]).is_err(),
            "an envelope truncated to {cut} bytes decoded"
        );
    }
    for bytes in [
        b"null".as_slice(),
        b"[]".as_slice(),
        b"true".as_slice(),
        b"not-json".as_slice(),
        &[0xff, 0xfe, 0xfd],
    ] {
        assert!(serde_json::from_slice::<GuestRequest<StatusRequest>>(bytes).is_err());
    }
}

#[test]
fn unknown_envelope_and_payload_fields_are_refused() {
    let mut envelope = serde_json::to_value(request()).expect("serialize");
    envelope["extra"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<GuestRequest<StatusRequest>>(envelope).is_err());

    let mut payload = serde_json::to_value(request()).expect("serialize");
    payload["request"]["extra"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<GuestRequest<StatusRequest>>(payload).is_err());
}

#[test]
fn a_guest_path_cannot_escape_its_root() {
    let root = GuestRoot::workspace();
    assert!(GuestPath::parse(&root, "/workspace/src/main.rs").is_ok());
    assert!(GuestPath::parse(&root, "/workspace").is_ok());
    assert_eq!(
        GuestPath::parse(&root, "/workspace/../etc/passwd"),
        Err(GuestPathError::Traversal)
    );
    assert_eq!(
        GuestPath::parse(&root, "/etc/passwd"),
        Err(GuestPathError::OutsideRoot)
    );
    assert_eq!(
        GuestPath::parse(&root, "workspace/x"),
        Err(GuestPathError::Relative)
    );
    assert_eq!(
        GuestPath::parse(&root, "/workspace/a\0b"),
        Err(GuestPathError::ControlByte)
    );
    assert_eq!(GuestPath::parse(&root, ""), Err(GuestPathError::Length));
    assert_eq!(
        GuestPath::parse(&root, "/workspace-other/x"),
        Err(GuestPathError::OutsideRoot)
    );
}

#[test]
fn a_search_pattern_is_enumerated_and_bounded() {
    let literal = SearchPattern::Literal {
        needle: "needle".to_owned(),
        case_sensitive: true,
    };
    assert!(literal.is_bounded());
    let oversize = SearchPattern::Regex {
        expression: "x".repeat(SearchPattern::MAX_BYTES + 1),
        case_sensitive: false,
    };
    assert!(!oversize.is_bounded());
    assert!(
        !SearchPattern::Glob {
            glob: String::new()
        }
        .is_bounded()
    );
}

#[test]
fn a_runtime_receipt_must_account_for_its_whole_interval() {
    let receipt = |running: u64, suspended: u64| RuntimeReceipt {
        generation: generation(),
        shape: ComputeSize::Gb1,
        running_ms: running,
        suspended_ms: suspended,
        from: Timestamp::from_unix_millis(1_785_501_296_000).expect("timestamp"),
        to: Timestamp::from_unix_millis(1_785_501_306_000).expect("timestamp"),
        snapshot_bytes: None,
        transmit_bytes: None,
    };
    assert!(receipt(4_000, 6_000).validate().is_ok());
    assert_eq!(
        receipt(4_000, 5_000).validate(),
        Err(ReceiptError::UnexplainedRemainder)
    );
    assert_eq!(
        receipt(4_000, 7_000).validate(),
        Err(ReceiptError::UnexplainedRemainder)
    );
}

#[test]
fn true_idle_is_brain_authoritative_not_guest_silence() {
    let mut evidence = TrueIdleEvidence {
        generation: generation(),
        activity_revision: 17,
        admitted: 0,
        queued: 0,
        open: 0,
        keepalive_lease: None,
        observed_at: Timestamp::from_unix_millis(1_785_501_296_789).expect("timestamp"),
    };
    assert!(evidence.is_true_idle());
    evidence.open = 1;
    assert!(!evidence.is_true_idle());
    evidence.open = 0;
    evidence.keepalive_lease = Some(KeepaliveLease {
        lease_id: "lease-1".to_owned(),
        expires_at: Timestamp::from_unix_millis(1_785_501_396_789).expect("timestamp"),
    });
    assert!(!evidence.is_true_idle());
}

#[test]
fn no_guest_reported_usage_type_exists() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for module in ["rpc.rs", "operation.rs"] {
        let text = std::fs::read_to_string(root.join(module)).expect("read module");
        for banned in ["RuntimeReceipt", "millicpu", "byte_ms", "billable"] {
            assert!(
                !text.contains(banned),
                "`{module}` mentions `{banned}`; billing evidence must not be guest-reachable"
            );
        }
    }
}
