//! `central-control-api` route and probe conformance evidence.

#[test]
fn both_internal_probes_are_mounted_and_readiness_can_refuse() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("aex_central_http::health::router"));
    assert!(source.contains("Probes::NONE"));
}

#[test]
fn the_composition_admits_before_any_client_is_opened() {
    let source = include_str!("../src/main.rs");
    let admit = source
        .find("capability::admit")
        .expect("`central-control-api` runs the composition check");
    let listen = source
        .find("lambda_http::run")
        .expect("`central-control-api` serves");
    assert!(
        admit < listen,
        "the capability check must precede the listener"
    );
}
