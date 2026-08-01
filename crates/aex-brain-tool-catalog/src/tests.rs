use crate::catalog::{
    ArgumentError, BUILTIN_CATALOG_DIGEST, builtin_catalog_bytes, builtin_entries, select_effect,
    validate_arguments,
};
use crate::control::{
    ControlFailure, apply_todo_write, prepare_submit_result, prepare_wait, render_todo_read,
};
use crate::manifest::{
    ApprovalPolicy, CatalogViolation, CredentialClass, Determinism, EgressClass, EntryState,
    RecoveryClass, ToolBoundary, ToolBounds, ToolDescriptor, ToolManifestEntry, UsageDimensionSet,
    validate_entry,
};
use crate::manifest::{CatalogCanonicalError, ToolName, ToolNameError, canonical_catalog_bytes};
use crate::readiness::{
    BuiltinSelection, CapabilitySet, ExecutorRegistry, ReadinessFailure, ReadinessInput,
    ResolvedSecretNames, advertise,
};
use crate::signature::{
    CatalogSignature, CatalogVerifyError, SigningKeyId, TrustStore, VerificationKey,
    catalog_signing_input, verify_catalog_signature,
};
use crate::wire_pending::{EffectClass, ExecutorRoute};
use aex_wire::CanonicalJson;
use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use sha2::Digest as _;

#[test]
fn tool_name_accepts_the_provider_intersection_grammar() {
    let name = ToolName::parse("AEX_tool-19").expect("valid provider name");
    assert_eq!(name.as_str(), "AEX_tool-19");

    let boundary = "a".repeat(ToolName::MAX_BYTES);
    assert_eq!(
        ToolName::parse(&boundary)
            .expect("the exact maximum is valid")
            .as_str(),
        boundary
    );
}

#[test]
fn tool_name_rejects_every_invalid_name_class() {
    assert_eq!(ToolName::parse(""), Err(ToolNameError::Empty));
    assert_eq!(
        ToolName::parse(&"a".repeat(ToolName::MAX_BYTES + 1)),
        Err(ToolNameError::TooLong {
            bytes: ToolName::MAX_BYTES + 1
        })
    );
    assert_eq!(
        ToolName::parse("read.file"),
        Err(ToolNameError::IllegalByte { at: 4, byte: b'.' })
    );
    assert_eq!(
        ToolName::parse("read file"),
        Err(ToolNameError::IllegalByte { at: 4, byte: b' ' })
    );
    assert_eq!(
        ToolName::parse("mcp__server__tool"),
        Err(ToolNameError::ReservedPrefix)
    );
}

#[test]
fn mcp_names_are_namespaced_without_truncation() {
    let name = ToolName::namespaced_mcp("search", "query").expect("valid MCP name");
    assert_eq!(name.as_str(), "mcp__search__query");

    assert_eq!(
        ToolName::namespaced_mcp(&"s".repeat(60), "query"),
        Err(ToolNameError::TooLong { bytes: 72 })
    );
}

#[test]
fn catalog_canonicalization_is_member_order_independent() {
    let left: serde_json::Value = serde_json::from_str(
        r#"{"name":"read_file","bounds":{"timeoutMs":60000,"maxResultBytes":1000000}}"#,
    )
    .expect("fixture is JSON");
    let right: serde_json::Value = serde_json::from_str(
        r#"{"bounds":{"maxResultBytes":1000000,"timeoutMs":60000},"name":"read_file"}"#,
    )
    .expect("fixture is JSON");

    assert_eq!(
        canonical_catalog_bytes(&left).expect("left canonicalizes"),
        canonical_catalog_bytes(&right).expect("right canonicalizes")
    );
}

#[test]
fn catalog_canonicalization_rejects_floats_at_any_depth() {
    let descriptor = serde_json::json!({
        "name": "broken",
        "inputSchema": {
            "type": "object",
            "properties": {"count": {"type": "number", "minimum": 1.5}}
        }
    });

    assert_eq!(
        canonical_catalog_bytes(&descriptor),
        Err(CatalogCanonicalError::FloatingPoint {
            pointer: "/inputSchema/properties/count/minimum".to_owned()
        })
    );
}

#[test]
fn catalog_signature_verifies_only_the_exact_digest_and_key() {
    let now = Timestamp::from_unix_millis(1_000).expect("fixture timestamp");
    let expires = Timestamp::from_unix_millis(2_000).expect("fixture timestamp");
    let (key_pair, verification_key) = signing_fixture(expires);
    let trust = TrustStore::new(vec![verification_key]).expect("unique trust store");
    let digest = hash(b"original catalog body");
    let input = catalog_signing_input(1, 1, &digest);
    let signature: Signature = key_pair.sign(&input);
    let signature = CatalogSignature::from_bytes(
        SigningKeyId::parse("catalog-key-1").expect("fixture key id"),
        &normalize_low_s(&signature.to_bytes()),
    )
    .expect("fixed-width signature");

    verify_catalog_signature(1, 1, &digest, &signature, &trust, now).expect("valid signature");

    let mutated_digest = hash(b"mutated catalog body");
    assert_eq!(
        verify_catalog_signature(1, 1, &mutated_digest, &signature, &trust, now),
        Err(CatalogVerifyError::InvalidSignature)
    );

    let unknown = CatalogSignature::from_bytes(
        SigningKeyId::parse("unknown-key").expect("fixture key id"),
        signature.as_bytes(),
    )
    .expect("fixed-width signature");
    assert_eq!(
        verify_catalog_signature(1, 1, &digest, &unknown, &trust, now),
        Err(CatalogVerifyError::UnknownKey {
            key_id: "unknown-key".to_owned()
        })
    );
}

#[test]
fn catalog_signature_rejects_expiry_malleability_and_bad_length() {
    let expires = Timestamp::from_unix_millis(2_000).expect("fixture timestamp");
    let (key_pair, verification_key) = signing_fixture(expires);
    let trust = TrustStore::new(vec![verification_key]).expect("unique trust store");
    let digest = hash(b"catalog");
    let raw: Signature = key_pair.sign(&catalog_signing_input(1, 2, &digest));
    let low = normalize_low_s(&raw.to_bytes());
    let signature = CatalogSignature::from_bytes(
        SigningKeyId::parse("catalog-key-1").expect("fixture key id"),
        &low,
    )
    .expect("fixed-width signature");

    assert_eq!(
        verify_catalog_signature(
            1,
            2,
            &digest,
            &signature,
            &trust,
            Timestamp::from_unix_millis(2_001).expect("fixture timestamp")
        ),
        Err(CatalogVerifyError::KeyExpired {
            key_id: "catalog-key-1".to_owned()
        })
    );

    let mut high = low;
    high[32..].copy_from_slice(&subtract_from_order(&low[32..]));
    let high = CatalogSignature::from_bytes(
        SigningKeyId::parse("catalog-key-1").expect("fixture key id"),
        &high,
    )
    .expect("fixed-width signature");
    assert_eq!(
        verify_catalog_signature(
            1,
            2,
            &digest,
            &high,
            &trust,
            Timestamp::from_unix_millis(1_999).expect("fixture timestamp")
        ),
        Err(CatalogVerifyError::MalleableSignature)
    );

    assert_eq!(
        CatalogSignature::from_bytes(
            SigningKeyId::parse("catalog-key-1").expect("fixture key id"),
            &[0; 63]
        ),
        Err(CatalogVerifyError::MalformedSignature { bytes: 63 })
    );
}

fn signing_fixture(expires: Timestamp) -> (SigningKey, VerificationKey) {
    let key_pair = SigningKey::from_bytes((&[7_u8; 32]).into()).expect("fixture key parses");
    let verification_key = VerificationKey {
        id: SigningKeyId::parse("catalog-key-1").expect("fixture key id"),
        public_key: key_pair
            .verifying_key()
            .to_sec1_point(false)
            .as_bytes()
            .to_vec()
            .into_boxed_slice(),
        not_after: expires,
    };
    (key_pair, verification_key)
}

fn hash(bytes: &[u8]) -> ContentHash {
    ContentHash::from_bytes(sha2::Sha256::digest(bytes).into())
}

fn normalize_low_s(raw: &[u8]) -> [u8; 64] {
    let mut signature: [u8; 64] = raw.try_into().expect("P-256 fixed signature");
    if signature[32..] > P256_HALF_ORDER[..] {
        let low = subtract_from_order(&signature[32..]);
        signature[32..].copy_from_slice(&low);
    }
    signature
}

fn subtract_from_order(s: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    let mut borrow = 0_u16;
    for index in (0..32).rev() {
        let minuend = u16::from(P256_ORDER[index]);
        let subtrahend = u16::from(s[index]) + borrow;
        if minuend >= subtrahend {
            out[index] = u8::try_from(minuend - subtrahend).expect("difference fits one byte");
            borrow = 0;
        } else {
            out[index] = u8::try_from(minuend + 256 - subtrahend)
                .expect("borrowed difference fits one byte");
            borrow = 1;
        }
    }
    out
}

const P256_ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];
const P256_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0x80, 0x00, 0x00, 0x00, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xde, 0x73, 0x7d, 0x56, 0xd3, 0x8b, 0xcf, 0x42, 0x79, 0xdc, 0xe5, 0x61, 0x7e, 0x31, 0x92, 0xa8,
];

#[test]
fn entry_validation_has_one_guard_for_each_structural_invariant() {
    let valid = valid_entry();
    validate_entry(&valid).expect("fixture is coherent");

    let mut entry = valid.clone();
    entry.descriptor.route = ExecutorRoute::ManagedWeb;
    assert!(matches!(
        validate_entry(&entry),
        Err(CatalogViolation::RouteBoundaryMismatch { .. })
    ));

    let mut entry = valid.clone();
    entry.descriptor.recovery = RecoveryClass::QueryDurableOperation;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::IncoherentRecovery)
    );

    let mut entry = valid.clone();
    entry.descriptor.effect = EffectClass::NonReplayable;
    entry.descriptor.recovery = RecoveryClass::InterruptOnAmbiguity;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::ZeroWeightExternalEffect)
    );

    let mut entry = valid.clone();
    entry.descriptor.bounds.max_context_bytes = 65_537;
    assert!(matches!(
        validate_entry(&entry),
        Err(CatalogViolation::ContextBound { .. })
    ));

    let mut entry = valid.clone();
    entry.descriptor.egress = EgressClass::ManagedInternet;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::DataTransferMeterMismatch)
    );

    let mut entry = valid.clone();
    entry.descriptor.boundary = ToolBoundary::HandsFilesystem;
    entry.descriptor.route = ExecutorRoute::HandsFilesystem;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::HandsMeterMustBeEmpty)
    );

    let mut entry = valid.clone();
    entry.descriptor.approval = ApprovalPolicy::Always;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::PureToolCannotAlwaysRequireApproval)
    );

    let mut entry = valid.clone();
    entry.descriptor.input_schema =
        CanonicalJson::parse(r#"{"type":"array","items":{"type":"string"}}"#)
            .expect("fixture schema is canonical JSON");
    assert!(matches!(
        validate_entry(&entry),
        Err(CatalogViolation::InvalidSchema { side: "input", .. })
    ));

    let mut entry = valid;
    entry.descriptor.effect = EffectClass::NonReplayable;
    entry.descriptor.recovery = RecoveryClass::InterruptOnAmbiguity;
    entry.descriptor.bounds.concurrency_weight = 1;
    assert_eq!(
        validate_entry(&entry),
        Err(CatalogViolation::DeterministicExternalEffect)
    );
}

fn valid_entry() -> ToolManifestEntry {
    let schema =
        CanonicalJson::parse(r#"{"additionalProperties":false,"properties":{},"type":"object"}"#)
            .expect("fixture schema is canonical JSON");
    ToolManifestEntry {
        descriptor: ToolDescriptor {
            name: ToolName::parse("todo_read").expect("fixture name"),
            title: "Read todos".into(),
            description: "Read the current folded todo state.".into(),
            input_schema: schema.clone(),
            result_schema: schema,
            boundary: ToolBoundary::BrainControl,
            route: ExecutorRoute::Control,
            effect: EffectClass::Pure,
            recovery: RecoveryClass::RecomputeFromInputHash,
            approval: ApprovalPolicy::Never,
            bounds: ToolBounds {
                max_input_bytes: 1_024,
                max_result_bytes: 65_536,
                max_context_bytes: 65_536,
                max_frame_bytes: 65_536,
                timeout_ms: 50,
                max_detached_ms: 0,
                concurrency_weight: 0,
            },
            usage: UsageDimensionSet::COMPUTE,
            egress: EgressClass::None,
            credential: CredentialClass::None,
            determinism: Determinism::Deterministic,
            variants: Vec::new(),
        },
        implementation_digest: hash(b"todo_read implementation"),
        required_capabilities: Vec::new(),
        state: EntryState::Active,
    }
}

#[test]
fn builtin_catalog_is_the_exact_clean_cut_launch_surface() {
    let entries = builtin_entries().expect("compiled catalog is valid");
    let names = entries
        .iter()
        .map(|entry| entry.descriptor.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, EXPECTED_BUILTINS);
    for retired in [
        "skills",
        "head",
        "tail",
        "wc",
        "ls",
        "subagent",
        "subagent_result",
        "code_execution",
    ] {
        assert!(
            !names.contains(&retired),
            "retired alias `{retired}` survived"
        );
    }

    for entry in entries.iter().filter(|entry| {
        matches!(
            entry.descriptor.boundary,
            ToolBoundary::HandsFilesystem
                | ToolBoundary::HandsDevelopment
                | ToolBoundary::HandsBrowser
        )
    }) {
        assert!(
            entry.descriptor.usage.is_empty(),
            "{}",
            entry.descriptor.name
        );
    }

    let run_command = entries
        .iter()
        .find(|entry| entry.descriptor.name.as_str() == "run_command")
        .expect("run_command descriptor");
    assert_eq!(run_command.descriptor.variants.len(), 2);
    assert_eq!(
        run_command.descriptor.variants[0].effect,
        EffectClass::NonReplayable
    );
    assert_eq!(
        run_command.descriptor.variants[1].effect,
        EffectClass::DurableDetached
    );

    let bytes = builtin_catalog_bytes().expect("catalog canonicalizes");
    let digest = hash(&bytes).to_string();
    assert_eq!(digest, BUILTIN_CATALOG_DIGEST);
    insta::assert_snapshot!(
        "builtin_catalog",
        String::from_utf8(bytes).expect("catalog is UTF-8")
    );
}

const EXPECTED_BUILTINS: [&str; 33] = [
    "apply_patch",
    "browser_control",
    "browser_launch",
    "browser_result",
    "browser_status",
    "create_subagent",
    "dequeue_subagent",
    "edit_file",
    "get_subagent",
    "git",
    "glob",
    "grep",
    "install_packages",
    "list_dir",
    "list_subagents",
    "process_output",
    "process_status",
    "process_stop",
    "read_file",
    "read_subagent_history",
    "request_approval",
    "run_code",
    "run_command",
    "send_subagent_message",
    "stop_subagent",
    "submit_result",
    "todo_read",
    "todo_write",
    "wait",
    "wait_subagents",
    "web_fetch",
    "web_search",
    "write_file",
];

#[test]
fn generated_argument_contracts_reject_hostile_shapes() {
    let entries = builtin_entries().expect("compiled catalog");
    let create = entries
        .iter()
        .find(|entry| entry.descriptor.name.as_str() == "create_subagent")
        .expect("create_subagent descriptor");

    for invalid in [
        serde_json::json!({"prompt":"work","model":"only-model"}),
        serde_json::json!({"prompt":"work","provider":"openai"}),
        serde_json::json!({"prompt":"work","unexpected":true}),
    ] {
        assert!(matches!(
            validate_arguments(create, &invalid),
            Err(ArgumentError::Schema { .. })
        ));
    }
    for valid in [
        serde_json::json!({"prompt":"work"}),
        serde_json::json!({"prompt":"work","provider":"openai","model":"gpt-test"}),
    ] {
        validate_arguments(create, &valid).expect("valid provider/model pairing");
    }
}

#[test]
fn run_command_effect_is_selected_from_validated_arguments() {
    let entries = builtin_entries().expect("compiled catalog");
    let run = entries
        .iter()
        .find(|entry| entry.descriptor.name.as_str() == "run_command")
        .expect("run_command descriptor");

    let attached = validate_arguments(run, &serde_json::json!({"command":"true"}))
        .expect("attached command validates");
    assert_eq!(
        select_effect(run, &attached).expect("attached variant"),
        (
            EffectClass::NonReplayable,
            RecoveryClass::InterruptOnAmbiguity,
            0
        )
    );

    let detached = validate_arguments(
        run,
        &serde_json::json!({"argv":["sleep","1"],"background":true}),
    )
    .expect("detached command validates");
    assert_eq!(
        select_effect(run, &detached).expect("detached variant"),
        (
            EffectClass::DurableDetached,
            RecoveryClass::QueryDurableOperation,
            43_200_000
        )
    );

    assert!(matches!(
        validate_arguments(run, &serde_json::json!({"command":"true","argv":["true"]})),
        Err(ArgumentError::Schema { .. })
    ));
}

#[test]
fn todo_control_tools_are_full_replacement_and_fold_pure() {
    let empty = CanonicalJson::parse(r#"{"todos":[]}"#).expect("fixture JSON");
    let (state, result) = apply_todo_write(&empty).expect("empty replacement is valid");
    assert_eq!(result.accepted, 0);
    assert_eq!(
        render_todo_read(None).expect("no prior write is empty"),
        render_todo_read(Some(&state)).expect("written empty state")
    );

    let args = CanonicalJson::parse(
        r#"{"todos":[{"activeForm":"Testing","content":"Test it","status":"in_progress"}]}"#,
    )
    .expect("fixture JSON");
    let first = apply_todo_write(&args).expect("todo replacement");
    let second = apply_todo_write(&args).expect("same replacement");
    assert_eq!(first, second, "same input must be byte-stable");
    assert_eq!(first.1.counts.in_progress, 1);

    let too_many = serde_json::json!({
        "todos": (0..201).map(|index| serde_json::json!({
            "content": format!("todo {index}"),
            "status": "pending",
            "activeForm": "Working"
        })).collect::<Vec<_>>()
    });
    let too_many = CanonicalJson::from_value(&too_many).expect("fixture JSON");
    assert_eq!(
        apply_todo_write(&too_many),
        Err(ControlFailure::InvalidArgument(
            "todos must contain at most 200 items"
        ))
    );
}

#[test]
fn wait_rejects_out_of_budget_instead_of_clamping() {
    let effect = hash(b"effect");
    assert_eq!(
        prepare_wait(0, 60_000, &effect),
        Err(ControlFailure::InvalidArgument(
            "seconds must be in 1..=86400"
        ))
    );
    assert_eq!(
        prepare_wait(86_401, 100_000_000, &effect),
        Err(ControlFailure::InvalidArgument(
            "seconds must be in 1..=86400"
        ))
    );
    assert_eq!(
        prepare_wait(61, 60_000, &effect),
        Err(ControlFailure::WaitExceedsRemainingBudget {
            requested_ms: 61_000,
            remaining_ms: 60_000
        })
    );
    assert_eq!(
        prepare_wait(60, 60_000, &effect).expect("exact remaining budget"),
        prepare_wait(60, 60_000, &effect).expect("same effect identity")
    );
}

#[test]
fn submit_result_digest_is_canonical_and_budgeted_without_truncation() {
    let left =
        CanonicalJson::parse(r#"{"status":"success","summary":"done","data":{"b":2,"a":1}}"#)
            .expect("fixture JSON");
    let right =
        CanonicalJson::parse(r#"{"data":{"a":1,"b":2},"summary":"done","status":"success"}"#)
            .expect("fixture JSON");
    assert_eq!(
        prepare_submit_result(&left, 1_000).expect("within retained budget"),
        prepare_submit_result(&right, 1_000).expect("same canonical document")
    );

    let large =
        serde_json::json!({"status":"success","summary":"done","data":{"body":"x".repeat(33_000)}});
    let large = CanonicalJson::from_value(&large).expect("fixture JSON");
    let prepared = prepare_submit_result(&large, 40_000).expect("large result is stored by ref");
    assert!(prepared.store_external);
    assert!(!prepared.truncated);
    assert_eq!(
        prepare_submit_result(&large, 32_000),
        Err(ControlFailure::ResultBytesExhausted {
            required: large.as_bytes().len() as u64,
            remaining: 32_000
        })
    );
}

#[test]
fn advertisement_is_exactly_one_ready_executor_and_optional_authority() {
    let entries = builtin_entries().expect("compiled catalog");
    let executors = ready_executors();
    let capabilities = CapabilitySet::new([("hands.browser", 1)]);
    let secrets = ResolvedSecretNames::new(["aex_web_search"]).expect("secret names");
    let advertised = advertise(ReadinessInput {
        entries: &entries,
        executors: &executors,
        capabilities: &capabilities,
        secrets: &secrets,
        selection: &BuiltinSelection::Default,
        approval_required: &[],
    })
    .expect("all routes ready");
    assert_eq!(advertised.entries.len(), 33);

    let no_search = ResolvedSecretNames::default();
    let advertised = advertise(ReadinessInput {
        entries: &entries,
        executors: &executors,
        capabilities: &capabilities,
        secrets: &no_search,
        selection: &BuiltinSelection::Default,
        approval_required: &[],
    })
    .expect("absent optional search authority excludes the tool");
    assert_eq!(advertised.entries.len(), 32);
    assert!(!advertised.contains("web_search"));

    let no_browser = CapabilitySet::default();
    let advertised = advertise(ReadinessInput {
        entries: &entries,
        executors: &executors,
        capabilities: &no_browser,
        secrets: &secrets,
        selection: &BuiltinSelection::Default,
        approval_required: &[],
    })
    .expect("absent optional browser capability excludes four tools");
    assert_eq!(advertised.entries.len(), 29);
}

#[test]
fn advertisement_fails_closed_on_ambiguous_missing_and_duplicate_routes() {
    let entries = builtin_entries().expect("compiled catalog");
    let capabilities = CapabilitySet::new([("hands.browser", 1)]);
    let secrets = ResolvedSecretNames::new(["aex_web_search"]).expect("secret names");

    let mut ambiguous = ready_executors();
    ambiguous.declare_ready(ExecutorRoute::ManagedWeb);
    assert!(matches!(
        advertise(ReadinessInput {
            entries: &entries,
            executors: &ambiguous,
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &BuiltinSelection::Default,
            approval_required: &[],
        }),
        Err(ReadinessFailure::AmbiguousRoute {
            route: ExecutorRoute::ManagedWeb,
            candidates: 2,
            ..
        })
    ));

    let mut missing = ready_executors();
    missing.remove(ExecutorRoute::ManagedWeb);
    assert!(matches!(
        advertise(ReadinessInput {
            entries: &entries,
            executors: &missing,
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &BuiltinSelection::Default,
            approval_required: &[],
        }),
        Err(ReadinessFailure::NoReadyExecutor {
            route: ExecutorRoute::ManagedWeb,
            ..
        })
    ));

    let mut duplicate = entries.clone();
    duplicate.push(entries[0].clone());
    assert!(matches!(
        advertise(ReadinessInput {
            entries: &duplicate,
            executors: &ready_executors(),
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &BuiltinSelection::Default,
            approval_required: &[],
        }),
        Err(ReadinessFailure::DuplicateName { .. })
    ));
}

#[test]
fn selection_and_approval_names_are_total() {
    let entries = builtin_entries().expect("compiled catalog");
    let executors = ready_executors();
    let capabilities = CapabilitySet::new([("hands.browser", 1)]);
    let secrets = ResolvedSecretNames::new(["aex_web_search"]).expect("secret names");
    let selection = BuiltinSelection::Exact(vec![
        ToolName::parse("todo_read").expect("fixture tool name"),
    ]);
    let advertised = advertise(ReadinessInput {
        entries: &entries,
        executors: &executors,
        capabilities: &capabilities,
        secrets: &secrets,
        selection: &selection,
        approval_required: &["todo_read"],
    })
    .expect("exact selection");
    assert_eq!(advertised.entries.len(), 1);

    let unknown = BuiltinSelection::Exact(vec![
        ToolName::parse("not_a_tool").expect("grammar-valid fixture"),
    ]);
    assert!(matches!(
        advertise(ReadinessInput {
            entries: &entries,
            executors: &executors,
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &unknown,
            approval_required: &[],
        }),
        Err(ReadinessFailure::UnknownSelection { .. })
    ));

    assert!(matches!(
        advertise(ReadinessInput {
            entries: &entries,
            executors: &executors,
            capabilities: &capabilities,
            secrets: &secrets,
            selection: &BuiltinSelection::Default,
            approval_required: &["not_a_tool"],
        }),
        Err(ReadinessFailure::ApprovalPolicyNamesUnknown { .. })
    ));
}

fn ready_executors() -> ExecutorRegistry {
    ExecutorRegistry::new([
        ExecutorRoute::Control,
        ExecutorRoute::Park,
        ExecutorRoute::SubagentScheduler,
        ExecutorRoute::ManagedWeb,
        ExecutorRoute::Mcp,
        ExecutorRoute::HandsFilesystem,
        ExecutorRoute::HandsDevelopment,
        ExecutorRoute::HandsBrowser,
        ExecutorRoute::RegisteredCustom,
    ])
}
