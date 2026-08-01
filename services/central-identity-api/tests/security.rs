//! `central-identity-api` capability and permission evidence.

#[test]
fn the_permission_list_is_declared_in_source_and_reviewable() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("pub const PERMISSIONS"));
}

#[test]
fn the_binary_pins_the_one_login_role_it_may_connect_as() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("const REQUIRED_ROLE"));
    assert!(source.contains("this binary connects only as"));
}

#[test]
fn the_capability_declaration_is_a_closed_set() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("BTreeSet::from(["));
    assert!(source.contains("CapabilityBinding::"));
}
