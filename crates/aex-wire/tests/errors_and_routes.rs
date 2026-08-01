//! The error and route registries, asserted against the corpus and against
//! themselves.
//!
//! The route surface is the part twelve peer streams build on, so the invariants
//! here are the ones a middleware stack is allowed to assume: `ROUTES` is
//! indexed by `RouteId`, every template round-trips through `match_route`, and
//! every route's precedence obligations are internally consistent.

use std::collections::BTreeSet;

use aex_wire::error::{ErrorClass, ErrorCode, ObservedErrorCode, PrecedenceStage, WireError};
use aex_wire::idempotency::IdempotencyKind;
use aex_wire::models::TelemetryGapReason;
use aex_wire::routes::{
    BodyClass, EtagPolicy, Plane, ROUTES, RouteId, TransportKind, match_route, route,
};
use aex_wire::testing::corpus;
use aex_wire::types::{HttpMethod, RequestId};

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ErrorCase {
    /// The wire code.
    code: String,
    /// The HTTP status it renders at.
    status: u16,
    /// Whether an identical retry can succeed.
    retryable: bool,
    /// The failure family.
    class: String,
    /// The precedence stage that may emit it.
    precedence_stage: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BindingCase {
    /// The `operationId`.
    operation_id: String,
    /// Which plane serves it.
    plane: String,
    /// HTTP method.
    method: String,
    /// A concrete path built from the template.
    path: String,
    /// The expected path bindings.
    bindings: std::collections::BTreeMap<String, String>,
}

fn class_key(class: ErrorClass) -> String {
    serde_json::to_value(class)
        .expect("serialize")
        .as_str()
        .expect("string")
        .to_owned()
}

fn stage_key(stage: PrecedenceStage) -> String {
    serde_json::to_value(stage)
        .expect("serialize")
        .as_str()
        .expect("string")
        .to_owned()
}

#[test]
fn the_error_corpus_covers_exactly_the_closed_vocabulary() {
    let cases: Vec<ErrorCase> = corpus::read_jsonl("errors/cases.jsonl");
    let covered: BTreeSet<&str> = cases.iter().map(|case| case.code.as_str()).collect();
    assert_eq!(
        covered.len(),
        cases.len(),
        "`errors/cases.jsonl` has a duplicate code"
    );
    for code in ErrorCode::ALL {
        assert!(
            covered.contains(code.as_str()),
            "no corpus case for `{}`",
            code.as_str()
        );
    }
    for case in &cases {
        let code = ErrorCode::parse(&case.code)
            .unwrap_or_else(|| panic!("corpus names unknown code `{}`", case.code));
        assert_eq!(code.http_status(), case.status, "{}", case.code);
        assert_eq!(code.retryable(), case.retryable, "{}", case.code);
        assert_eq!(class_key(code.class()), case.class, "{}", case.code);
        assert_eq!(
            stage_key(code.precedence_stage()),
            case.precedence_stage,
            "{}",
            case.code
        );
    }
}

#[test]
fn every_error_code_renders_one_envelope() {
    let request_id = RequestId::parse("req-test-0001").expect("request id");
    for code in ErrorCode::ALL {
        let (status, body, retry) = WireError::new(*code).into_response_parts(&request_id, None);
        assert_eq!(status, code.http_status());
        assert_eq!(body.error.retryable, code.retryable());
        assert_eq!(body.error.code, ObservedErrorCode::Known(*code));
        assert_eq!(body.error.message, code.default_message());
        assert!(retry.is_none());
        let json = serde_json::to_string(&body).expect("serialize");
        let decoded: aex_wire::error::ApiError = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded.error.code, body.error.code);
    }
}

#[test]
fn an_unrecognized_code_decodes_without_becoming_a_known_one() {
    let observed: ObservedErrorCode =
        serde_json::from_str("\"a_code_from_a_newer_server\"").expect("decode");
    assert_eq!(observed.known(), None);
    assert_eq!(observed.as_str(), "a_code_from_a_newer_server");
    assert_eq!(
        serde_json::to_string(&observed).expect("serialize"),
        "\"a_code_from_a_newer_server\""
    );
}

#[test]
fn precedence_stages_are_declared_in_evaluation_order() {
    for pair in PrecedenceStage::ALL.windows(2) {
        assert!(
            pair[0].order() < pair[1].order(),
            "{:?} must precede {:?}",
            pair[0],
            pair[1]
        );
    }
    // An authentication failure must never be reported after a domain-state one:
    // that ordering is what stops a paused caller being shown a grant.
    assert!(
        ErrorCode::Unauthenticated.precedence_stage().order()
            < ErrorCode::AccountPaused.precedence_stage().order()
    );
    assert!(
        ErrorCode::AccountPaused.precedence_stage().order()
            < ErrorCode::IdempotencyConflict.precedence_stage().order()
    );
    assert!(
        ErrorCode::WrongWorkspaceRegion.precedence_stage().order()
            < ErrorCode::NotFound.precedence_stage().order()
    );
}

#[test]
fn the_route_table_is_indexed_by_route_id_and_has_the_pinned_arity() {
    assert_eq!(ROUTES.len(), RouteId::ALL.len());
    for (index, descriptor) in ROUTES.iter().enumerate() {
        assert_eq!(descriptor.id as usize, index, "{}", descriptor.operation_id);
        assert_eq!(route(descriptor.id).operation_id, descriptor.operation_id);
        assert_eq!(descriptor.id.as_str(), descriptor.operation_id);
        assert_eq!(
            RouteId::parse(descriptor.operation_id),
            Some(descriptor.id),
            "{}",
            descriptor.operation_id
        );
    }
    let central = ROUTES.iter().filter(|r| r.plane == Plane::Central).count();
    let regional = ROUTES.iter().filter(|r| r.plane == Plane::Regional).count();
    assert_eq!(central, 27, "central plane arity");
    assert_eq!(regional, 119, "regional plane arity");
    assert_eq!(central + regional, 146, "total public operation arity");
}

#[test]
fn the_session_lifecycle_vocabulary_is_clone_trash_restore_purge() {
    // R-DELETE supersedes `fork` and `delete`. Prelaunch clean cut means the old
    // names are gone rather than aliased, so their absence is asserted here: an
    // alias would let a generated client keep calling a verb whose semantics no
    // longer exist.
    for retired in ["session_fork", "session_delete"] {
        assert_eq!(RouteId::parse(retired), None, "`{retired}` must be gone");
        assert!(
            !ROUTES.iter().any(|r| r.operation_id == retired),
            "`{retired}` must be gone"
        );
    }
    for (operation, template) in [
        ("session_clone", "/api/sessions/{sessionId}/clones"),
        ("session_trash", "/api/sessions/{sessionId}/trashes"),
        ("session_restore", "/api/sessions/{sessionId}/restores"),
        ("session_purge", "/api/sessions/{sessionId}/purges"),
    ] {
        let id = RouteId::parse(operation).unwrap_or_else(|| panic!("`{operation}` must exist"));
        let descriptor = route(id);
        assert_eq!(descriptor.template, template);
        assert_eq!(descriptor.method, HttpMethod::Post);
        assert_eq!(descriptor.success_status, 202);
        assert_eq!(descriptor.idempotency, IdempotencyKind::OperationId);
    }
    // Trash and purge are destructive controls a paused account must still
    // reach; restore is an ordinary mutation and is not exempt.
    assert!(route(RouteId::SessionTrash).pause_exempt);
    assert!(route(RouteId::SessionPurge).pause_exempt);
    assert!(!route(RouteId::SessionRestore).pause_exempt);
    assert!(!route(RouteId::SessionClone).pause_exempt);
}

#[test]
fn operation_ids_and_plane_routes_are_unique() {
    let ids: BTreeSet<&str> = ROUTES.iter().map(|r| r.operation_id).collect();
    assert_eq!(ids.len(), ROUTES.len(), "duplicate operationId");
    let mut pairs = BTreeSet::new();
    for descriptor in ROUTES {
        assert!(
            pairs.insert((descriptor.plane, descriptor.method, descriptor.template)),
            "duplicate route `{} {}`",
            descriptor.method,
            descriptor.template
        );
    }
}

#[test]
fn every_route_binds_through_the_matcher() {
    let cases: Vec<BindingCase> = corpus::read_jsonl("routes/bindings.jsonl");
    assert_eq!(cases.len(), ROUTES.len(), "one binding case per route");
    for case in &cases {
        let plane = if case.plane == "central" {
            Plane::Central
        } else {
            Plane::Regional
        };
        let method = HttpMethod::parse(&case.method).expect("method");
        let (matched, binding) = match_route(plane, method, &case.path)
            .unwrap_or_else(|| panic!("`{} {}` did not match", case.method, case.path));
        assert_eq!(
            matched.as_str(),
            case.operation_id,
            "`{} {}` matched the wrong route",
            case.method,
            case.path
        );
        assert_eq!(binding.len(), case.bindings.len());
        for (name, expected) in &case.bindings {
            assert_eq!(binding.get(name), Some(expected.as_str()), "{name}");
        }
    }
}

#[test]
fn the_matcher_refuses_near_misses() {
    assert!(match_route(Plane::Central, HttpMethod::Get, "/api/account").is_some());
    for path in [
        "/api/account/",
        "/api/Account",
        "api/account",
        "/api/account/extra",
        "/api",
    ] {
        assert!(
            match_route(Plane::Central, HttpMethod::Get, path).is_none(),
            "`{path}` must not match"
        );
    }
    // A central path is not reachable on the regional plane, and vice versa.
    assert!(match_route(Plane::Regional, HttpMethod::Get, "/api/account").is_none());
    assert!(match_route(Plane::Central, HttpMethod::Get, "/api/workspace").is_none());
}

#[test]
fn route_obligations_are_internally_consistent() {
    for descriptor in ROUTES {
        let operation = descriptor.operation_id;
        assert!(
            descriptor.template.starts_with("/api/"),
            "{operation} is not rooted at /api"
        );
        assert!(
            !descriptor.errors.is_empty(),
            "{operation} declares no errors"
        );
        assert!(
            descriptor.errors.windows(2).all(|pair| pair[0] < pair[1]),
            "{operation} declares errors out of order"
        );

        // A body class and a request schema imply each other.
        match descriptor.body_class {
            BodyClass::None => assert!(
                descriptor.request_schema.is_none(),
                "{operation} has a request schema but no body"
            ),
            BodyClass::AexJson => assert!(
                descriptor.request_schema.is_some(),
                "{operation} declares an AEX body with no schema"
            ),
            BodyClass::Otlp => assert!(
                descriptor.request_schema.is_none(),
                "{operation} declares an OTLP body and an AEX schema"
            ),
        }

        // 204 is the only bodyless success, and 202 is only ever an operation.
        if descriptor.response_schema.is_none() {
            assert_eq!(descriptor.success_status, 204, "{operation}");
        }
        if descriptor.success_status == 202 {
            assert_eq!(descriptor.response_schema, Some("Operation"), "{operation}");
            assert_eq!(
                descriptor.idempotency,
                IdempotencyKind::OperationId,
                "{operation} admits an operation without an operation identity"
            );
        }

        // A durable-operation admission is a POST that returns 202.
        if descriptor.idempotency == IdempotencyKind::OperationId {
            assert_eq!(descriptor.method, HttpMethod::Post, "{operation}");
            assert_eq!(descriptor.success_status, 202, "{operation}");
        }

        // A GET is safe to retry and never carries a replay identity.
        if descriptor.method == HttpMethod::Get {
            assert!(descriptor.safe_retry, "{operation}");
            assert_eq!(descriptor.idempotency, IdempotencyKind::None, "{operation}");
        }

        // An NDJSON route is a bounded typed read, so it is a POST with a body.
        if descriptor.transport == TransportKind::Ndjson {
            assert_eq!(descriptor.method, HttpMethod::Post, "{operation}");
            assert!(descriptor.request_schema.is_some(), "{operation}");
        }

        // `If-Match` is only meaningful where the resource has an entity tag.
        if descriptor.etag == EtagPolicy::RequiredIfMatch {
            assert!(
                matches!(descriptor.method, HttpMethod::Put | HttpMethod::Post),
                "{operation} requires If-Match on a read"
            );
        }

        // Every authenticated route names a scope; only the device flow does not.
        if descriptor.required_scope.is_none() {
            assert!(
                descriptor.alt_principal.is_some(),
                "{operation} has neither a scope nor an alternative principal"
            );
        }
    }
}

#[test]
fn pause_exempt_routes_are_exactly_the_declared_exemptions() {
    // Security revocation, stop, discard, destructive deletion, account and
    // workspace state, billing, and safe control reads. Anything else that
    // claims exemption is a bug in the fragment, not a policy question.
    for descriptor in ROUTES.iter().filter(|r| r.pause_exempt) {
        let operation = descriptor.operation_id;
        let exempt = operation.contains("revocation")
            || operation.contains("revoke")
            || operation.contains("delete")
            || operation.contains("trash")
            || operation.contains("purge")
            || operation.contains("discard")
            || operation.contains("stop")
            || operation.contains("abort")
            || operation.contains("operation")
            || operation.contains("billing")
            || operation.contains("account")
            || operation.contains("workspace")
            || operation.contains("usage")
            || operation.contains("device")
            || operation.contains("bootstrap");
        assert!(
            exempt,
            "{operation} claims a pause exemption it has no basis for"
        );
    }
}

#[test]
fn the_replay_expired_gap_reason_exists_but_no_contract_surface_produces_it() {
    // The reason stays in the vocabulary so a historical gap record still
    // decodes. Nothing in the contract can mint one: it is reachable only from a
    // streaming buffer that the launch architecture does not have.
    assert!(TelemetryGapReason::ALL.contains(&TelemetryGapReason::ReplayExpired));
    assert_eq!(TelemetryGapReason::ReplayExpired.as_str(), "replay_expired");
    let produced: BTreeSet<&str> = ROUTES
        .iter()
        .filter_map(|descriptor| descriptor.response_schema)
        .collect();
    assert!(
        produced.contains("TelemetryGap"),
        "the gap shape must still be readable"
    );
    for descriptor in ROUTES {
        assert!(
            descriptor.request_schema != Some("TelemetryGap"),
            "{} lets a caller submit a gap",
            descriptor.operation_id
        );
    }
}
