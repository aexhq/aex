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
