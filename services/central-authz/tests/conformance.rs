//! `central-authz` invocation and probe conformance evidence.

/// This binary is invoked, never routed as an HTTP proxy integration.
///
/// Regional assertion requests and API Gateway REQUEST-authorizer events both
/// reach the direct-invoke runtime. A mounted path would still be a route it can
/// never serve, so the source must not build a router or run an HTTP adapter.
#[test]
fn this_binary_mounts_no_route_because_it_can_serve_none() {
    let source = include_str!("../src/main.rs");
    assert!(
        !source.contains("lambda_http"),
        "an API Gateway proxy event never arrives at a directly invoked function"
    );
    assert!(
        !source.contains("health::router"),
        "a health route nothing can request is a route that cannot be served (RS-18)"
    );
    assert!(
        source.contains("lambda_runtime::run"),
        "the invoke handler is the surface this binary serves"
    );
}

/// The central edge's context now has one concrete producer.
#[test]
fn the_api_gateway_request_authorizer_is_composed_before_the_assertion_classifier() {
    let source = include_str!("../src/main.rs");
    let authorizer = include_str!("../src/authorizer.rs");
    let request = source
        .find("RequestInvocation::matches")
        .expect("the REQUEST-authorizer classifier is mounted");
    let assertion = source
        .find("Invocation::classify")
        .expect("the regional assertion classifier is mounted");
    assert!(request < assertion, "REQUEST events must not fall through");
    assert!(authorizer.contains("ApiGatewayV2CustomAuthorizerV2Request"));
    assert!(authorizer.contains("ApiGatewayCustomAuthorizerRequestTypeRequest"));
    assert!(authorizer.contains("CentralAuthorizerContext"));
    assert!(authorizer.contains("\"isAuthorized\": true"));
    assert!(authorizer.contains("\"policyDocument\""));
}

/// Readiness is a start-up gate rather than an endpoint.
///
/// With no path to report `503` on, "not ready" and "not running" are the same
/// state, and the honest one is the second.
#[test]
fn readiness_refuses_the_process_rather_than_answering_a_request() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("Probes::NONE"));
    let ready = source
        .find("RunError::NotReady")
        .expect("an unproven probe must stop the process");
    let listen = source
        .find("lambda_runtime::run")
        .expect("`central-authz` serves");
    assert!(
        ready < listen,
        "the readiness gate must precede the runtime"
    );
}

#[test]
fn the_composition_admits_before_any_client_is_opened() {
    let source = include_str!("../src/main.rs");
    let admit = source
        .find("capability::admit")
        .expect("`central-authz` runs the composition check");
    let client = source
        .find("aws_config::from_env")
        .expect("`central-authz` opens clients");
    let listen = source
        .find("lambda_runtime::run")
        .expect("`central-authz` serves");
    assert!(
        admit < client,
        "the capability check must precede the first client"
    );
    assert!(
        admit < listen,
        "the capability check must precede the runtime"
    );
}

/// The plaintext credential never reaches this plane.
///
/// A request names a credential by `(id, presentedDigest)`. The evidence that
/// nothing reads one here is the absence of any plaintext member, and both
/// request types are `deny_unknown_fields`, so a caller that sent a token would
/// get a decode failure rather than a silently ignored field.
#[test]
fn the_issue_path_never_names_a_plaintext_credential() {
    let source = include_str!("../src/issue.rs");
    assert!(
        source.contains("presented_digest"),
        "the digest is what a request carries"
    );
    for forbidden in ["request.credential", "request.token", "MintedSecret"] {
        assert!(!source.contains(forbidden), "`{forbidden}` must not appear");
    }
}
