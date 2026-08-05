//! `central-control-worker` route and probe conformance evidence.

#[test]
fn readiness_can_refuse_before_the_async_runtime_starts() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("aex_central_http::health::router"));
    assert!(source.contains("Probes::NONE"));
}

#[test]
fn the_composition_admits_before_any_client_is_opened() {
    let source = include_str!("../src/main.rs");
    let admit = source
        .find("capability::admit")
        .expect("`central-control-worker` runs the composition check");
    let listen = source
        .find("lambda_runtime::run")
        .expect("`central-control-worker` consumes events");
    assert!(
        admit < listen,
        "the capability check must precede the listener"
    );
}

#[test]
fn ses_is_a_delivery_dependency_not_a_cold_start_dependency() {
    let composition = include_str!("../src/main.rs");
    let runtime = include_str!("../src/runtime.rs");

    assert!(runtime.contains(".send_email()"));
    assert!(runtime.contains("release_outbox"));
    assert!(!composition.contains("get_email_identity"));
    assert!(!composition.contains("mail_identity"));
    assert!(!composition.contains("name: \"ses-identity\""));
}
