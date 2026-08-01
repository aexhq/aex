//! The hostile-input boundary.
//!
//! Hands is customer root, so every case here assumes the sender is trying to
//! get something past the decoder rather than merely getting it wrong.

use aex_hands_protocol::GuestRevision;
use aex_hands_protocol::lifecycle::{
    KeepaliveLease, ReceiptError, RuntimeReceipt, TrueIdleEvidence,
};
use aex_hands_protocol::operation::{GuestPath, GuestPathError, GuestRoot, SearchPattern};
use aex_hands_protocol::rpc::{
    Fence, GenerationBinding, GenerationExpectation, MessageDecodeError, StatusResponse,
    decode_agent_message,
};
use aex_internal_contracts::SchemaVersion;
use aex_wire::ids::{GenerationId, PrefixedId};
use aex_wire::types::{ComputeSize, Timestamp};

fn generation() -> GenerationId {
    GenerationId::parse("gen_01kyw2qa4ne00r40r40m30e209").expect("generation id")
}

fn other_generation() -> GenerationId {
    GenerationId::parse("gen_01kyw2qa4pew48j2gb1g6gw3rg").expect("generation id")
}

fn expectation() -> GenerationExpectation {
    GenerationExpectation {
        generation: generation(),
        min_fence: Fence(7),
        max_frame_bytes: 4096,
        schema_version: SchemaVersion::V1,
    }
}

fn frame(generation: GenerationId, fence: u64, version: u32) -> Vec<u8> {
    let response = serde_json::json!({
        "binding": GenerationBinding {
            schema_version: SchemaVersion(version),
            generation,
            fence: Fence(fence),
        },
        "status": "accepted",
        "operation": "01kyw2qa4ne00r40r40m30e209",
        "guestRevision": 1,
    });
    serde_json::to_vec(&response).expect("serialize")
}

#[test]
fn a_well_formed_frame_decodes() {
    let bytes = frame(generation(), 9, 1);
    let decoded: StatusResponse =
        decode_agent_message(&bytes, &expectation()).expect("a valid frame decodes");
    match decoded {
        StatusResponse::Accepted { guest_revision, .. } => {
            assert_eq!(guest_revision, GuestRevision(1));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn an_oversize_frame_is_refused_before_it_is_parsed() {
    let mut expectation = expectation();
    expectation.max_frame_bytes = 16;
    let bytes = frame(generation(), 9, 1);
    let error = decode_agent_message::<StatusResponse>(&bytes, &expectation)
        .expect_err("an oversize frame must be refused");
    assert!(
        matches!(error, MessageDecodeError::Oversize { limit: 16, .. }),
        "{error:?}"
    );
}

#[test]
fn a_stale_or_foreign_generation_never_reaches_the_payload() {
    let error =
        decode_agent_message::<StatusResponse>(&frame(other_generation(), 9, 1), &expectation())
            .expect_err("a foreign generation must be refused");
    assert!(
        matches!(error, MessageDecodeError::WrongGeneration { .. }),
        "{error:?}"
    );

    let error = decode_agent_message::<StatusResponse>(&frame(generation(), 6, 1), &expectation())
        .expect_err("a stale fence must be refused");
    assert!(
        matches!(error, MessageDecodeError::StaleFence { .. }),
        "{error:?}"
    );

    let error = decode_agent_message::<StatusResponse>(&frame(generation(), 9, 2), &expectation())
        .expect_err("an unknown envelope version must be refused");
    assert!(
        matches!(error, MessageDecodeError::UnsupportedVersion { found: 2 }),
        "{error:?}"
    );
}

#[test]
fn truncated_and_non_json_input_is_a_typed_error_not_a_panic() {
    for bytes in [
        b"".to_vec(),
        b"{".to_vec(),
        b"null".to_vec(),
        b"[1,2,3]".to_vec(),
        vec![0xff, 0xfe, 0xfd],
        b"{\"binding\":{}}".to_vec(),
    ] {
        let error = decode_agent_message::<StatusResponse>(&bytes, &expectation())
            .expect_err("hostile input must be refused");
        assert!(
            matches!(error, MessageDecodeError::Malformed { .. }),
            "{error:?}"
        );
    }
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
    // A prefix that merely starts with the root name is not inside it.
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
    assert!(
        !oversize.is_bounded(),
        "an unbounded pattern must be refused"
    );
    let empty = SearchPattern::Glob {
        glob: String::new(),
    };
    assert!(!empty.is_bounded());
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
    // A remainder over-charges if billed as running and under-charges if
    // dropped, so it is a hard error either way.
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
    assert!(!evidence.is_true_idle(), "an open connection is not idle");
    evidence.open = 0;

    evidence.keepalive_lease = Some(KeepaliveLease {
        lease_id: "lease-1".to_owned(),
        expires_at: Timestamp::from_unix_millis(1_785_501_396_789).expect("timestamp"),
    });
    assert!(
        !evidence.is_true_idle(),
        "a held keepalive lease is not idle regardless of guest silence"
    );
}

#[test]
fn no_guest_reported_usage_type_exists() {
    // H-BOUNDARY: customer root can falsify any in-guest counter, so "bill from
    // guest metrics" has to be unwritable rather than merely discouraged. The
    // only billable evidence lives in `lifecycle`, and nothing in `rpc` or
    // `operation` names a duration, a byte count that is billed, or a CPU figure.
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
