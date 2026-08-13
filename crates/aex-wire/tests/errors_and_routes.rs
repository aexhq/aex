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
fn an_api_key_replay_cannot_claim_to_recover_the_one_time_secret() {
    let code = ErrorCode::ApiKeySecretUnavailable;
    assert_eq!(code.http_status(), 409);
    assert!(!code.retryable());
    assert!(route(RouteId::ApiKeyCreate).declares(code));
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
fn the_route_table_is_indexed_by_route_id() {
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
}

#[test]
fn the_session_lifecycle_vocabulary_is_session_centric() {
    // The MVP has one eight-hour session and no public run, persistence, or
    // trash/restore lifecycle. Prelaunch clean cut means those old names are
    // gone rather than aliased: a compatibility alias would let a generated
    // client keep calling semantics that no longer exist.
    for retired in [
        "session_fork",
        "session_stop",
        "session_trash",
        "session_restore",
        "session_purge",
        "session_persist",
    ] {
        assert_eq!(RouteId::parse(retired), None, "`{retired}` must be gone");
        assert!(
            !ROUTES.iter().any(|r| r.operation_id == retired),
            "`{retired}` must be gone"
        );
    }

    let create = route(RouteId::SessionCreate);
    assert_eq!(create.template, "/api/sessions");
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(create.success_status, 201);
    assert_eq!(create.idempotency, IdempotencyKind::IdempotencyKey);
    assert!(!create.pause_exempt);
    assert!(create.declares(ErrorCode::AccountPaused));

    // Resume can increase active compute and therefore remains behind the
    // account-pause gate. Suspend, terminate, and irreversible deletion reduce
    // or destroy retained resources, so they remain reachable while paused.
    for (id, operation, template, pause_exempt) in [
        (
            RouteId::SessionResume,
            "session_resume",
            "/api/sessions/{sessionId}/resumptions",
            false,
        ),
        (
            RouteId::SessionSuspend,
            "session_suspend",
            "/api/sessions/{sessionId}/suspensions",
            true,
        ),
        (
            RouteId::SessionTerminate,
            "session_terminate",
            "/api/sessions/{sessionId}/terminations",
            true,
        ),
        (
            RouteId::SessionDelete,
            "session_delete",
            "/api/sessions/{sessionId}/deletions",
            true,
        ),
    ] {
        let descriptor = route(id);
        assert_eq!(descriptor.operation_id, operation);
        assert_eq!(RouteId::parse(operation), Some(id));
        assert_eq!(descriptor.template, template);
        assert_eq!(descriptor.method, HttpMethod::Post);
        assert_eq!(descriptor.success_status, 202);
        assert_eq!(descriptor.idempotency, IdempotencyKind::OperationId);
        assert_eq!(descriptor.pause_exempt, pause_exempt, "{operation}");
        assert_eq!(
            descriptor.declares(ErrorCode::AccountPaused),
            !pause_exempt,
            "{operation} must agree with the account-pause precedence gate"
        );
    }
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
            BodyClass::Binary => assert!(
                descriptor.request_schema.is_none(),
                "{operation} declares a binary body and an AEX schema"
            ),
        }

        // 204 is the only bodyless success, and 202 is only ever an operation.
        if descriptor.response_schema.is_none() {
            assert_eq!(
                descriptor.success_status,
                if descriptor.transport == TransportKind::Binary {
                    200
                } else {
                    204
                },
                "{operation}"
            );
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
        if descriptor.transport == TransportKind::Binary {
            assert_eq!(descriptor.method, HttpMethod::Get, "{operation}");
            assert!(descriptor.response_schema.is_none(), "{operation}");
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
    // Security revocation, current-work cancellation, stop, discard,
    // destructive deletion, account and workspace state, billing, safe control
    // reads, and the credential ceremony.
    // Anything else that claims exemption is a bug in the fragment, not a policy
    // question.
    //
    // The ceremony is exempt for the reason the pause exists: a paused account
    // is told to top up, and topping up happens in a browser the person must be
    // able to sign into. A pause that locked sign-in would be unrecoverable
    // without support.
    for descriptor in ROUTES.iter().filter(|r| r.pause_exempt) {
        let operation = descriptor.operation_id;
        let exempt = operation.starts_with("dashboard_session")
            || matches!(
                operation,
                "session_cancel"
                    | "session_suspend"
                    | "session_terminate"
                    | "session_files_live_list"
                    | "session_files_live_stat"
            )
            || operation.starts_with("session_files_live_download")
            || operation.contains("revocation")
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
