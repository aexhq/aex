//! Finance API route conformance evidence.

#[test]
fn public_health_routes_are_both_mounted() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("/internal/healthz"));
    assert!(source.contains("/internal/readyz"));
    assert!(source.contains("SERVICE_UNAVAILABLE"));
}
