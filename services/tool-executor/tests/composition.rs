//! What this deployable is allowed to link, and what it composes.
//!
//! The sibling of `aex-hands-agent`'s dependency-closure scans, and the same
//! reasoning: the reason this process exists is that the platform's vendor
//! credential should not sit in the process that also holds every tenant's
//! secrets. A dependency edge is how that stops being true, so the edge is what
//! is asserted rather than the intention.

const MANIFEST: &str = include_str!("../Cargo.toml");
const MAIN: &str = include_str!("../src/main.rs");

fn declared_dependencies() -> &'static str {
    MANIFEST
        .split("[dev-dependencies]")
        .next()
        .expect("a manifest has a first section")
}

#[test]
fn the_executor_links_no_tenant_custody_or_session_store() {
    // Each of these would put a customer's stored secrets, or the authority that
    // resolves them, inside the one process that holds the platform's own key.
    // `aex-brain-managed-web` reaches all of them through its `brain-adapter`
    // feature, which is why this crate takes it with `default-features = false`.
    let forbidden = [
        "aex-secret-custody-dynamodb",
        "aex-secret-keystore-dynamodb",
        "aex-secret-aws",
        "aex-session-dynamodb",
        "aex-brain-store-dynamodb",
        "aex-brain-provider-custody",
        "aex-brain-provider-gateway",
        "aex-registry-dynamodb",
        "aex-content-aws",
    ];
    let dependencies = declared_dependencies();
    for name in forbidden {
        assert!(
            !dependencies.contains(name),
            "the executor must not depend on `{name}`: the split exists so the \
             platform's vendor key is not in the process that reaches tenant secrets"
        );
    }
}

#[test]
fn the_brain_adapter_feature_stays_off() {
    let dependencies = declared_dependencies();
    let line = dependencies
        .lines()
        .find(|line| line.contains("aex-brain-managed-web"))
        .expect("the executor uses the managed web crate");
    assert!(
        line.contains("default-features = false"),
        "turning the default features back on relinks the tenant-BYOK credential \
         source and the Brain's executor: {line}"
    );
}

#[test]
fn the_composition_root_uses_no_in_memory_ceiling() {
    // `InMemoryOrganizationCeiling` exists so a test can obtain a `SpendPermit`
    // it cannot forge. A deployed process reaching for it would have a ceiling
    // that resets on every restart, which is a ceiling nobody enforces.
    assert!(
        !MAIN.contains("InMemoryOrganizationCeiling"),
        "the deployed composition must use the conditional-update ceiling"
    );
    assert!(
        MAIN.contains("DynamoOrganizationCeiling"),
        "the deployed composition must compose a ceiling at all"
    );
}

#[test]
fn the_composition_root_refuses_to_start_without_its_credential() {
    // A task that started, reported ready, and then failed every call at the
    // vendor would be indistinguishable from a vendor outage. The credential is
    // read before the listener is bound, so the failure is a start-up refusal an
    // operator sees once rather than a per-call error they have to correlate.
    let credential_at = MAIN
        .find("get_secret_value")
        .expect("the credential is read at start-up");
    let bind_at = MAIN
        .find("TcpListener::bind")
        .expect("the listener is bound at start-up");
    assert!(
        credential_at < bind_at,
        "the listener is bound before the credential is proved readable"
    );
}
