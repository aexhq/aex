//! Start-up, configuration and route-ownership facts for `regional-session-api`.

use std::collections::BTreeMap;

use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use regional_session_api::config::{self, Config};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (
            config::REGIONAL_API_URL,
            "https://eu-west-1.api.aex.test".to_owned(),
        ),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (config::SESSION_PORT, "8080".to_owned()),
        // Deliberately below the 30s ECS stop timeout: a drain deadline above
        // it never fires, because `SIGKILL` arrives first.
        (config::SESSION_DRAIN_DEADLINE_MS, "25000".to_owned()),
        (
            config::AUTHZ_FUNCTION_ARN,
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-authz".to_owned(),
        ),
        (
            config::AUTHZ_VERIFY_KEYS_PARAM,
            "/aex/dev/authz/verify-keys".to_owned(),
        ),
        (
            config::AUTHZ_PROJECTION_TABLE,
            "aex-dev-regional-authz-projection".to_owned(),
        ),
        (
            config::SESSION_TABLE,
            "aex-dev-session-authority".to_owned(),
        ),
        (config::WORK_TABLE, "aex-dev-regional-work".to_owned()),
        (config::CONTENT_TABLE, "aex-dev-regional-content".to_owned()),
        (
            config::REGISTRY_TABLE,
            "aex-dev-regional-registry".to_owned(),
        ),
        (
            config::SECRET_CUSTODY_TABLE,
            "aex-dev-regional-secret-custody".to_owned(),
        ),
        (
            config::RUNTIME_ACTIVITY_TABLE,
            "aex-dev-runtime-activity".to_owned(),
        ),
        (
            config::USAGE_QUERY_TABLE,
            "aex-dev-usage-query-projection".to_owned(),
        ),
        (
            config::CONTENT_BUCKET,
            "aex-dev-eu-west-1-content".to_owned(),
        ),
        (config::CONTENT_BUCKET_OWNER, "000000000000".to_owned()),
        (
            config::CONTENT_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"
                .to_owned(),
        ),
        (
            config::CURSOR_SIGNING_KEY_REF,
            "/aex/dev/regional/cursor-signing-key".to_owned(),
        ),
        (config::ASSERTION_CACHE_BYTES, "1048576".to_owned()),
        (config::MAX_JSON_BODY_BYTES, "65536".to_owned()),
        (config::MAX_PAGE_ITEMS, "100".to_owned()),
        (config::MAX_PAGE_BYTES, "1048576".to_owned()),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, RegionalHttpConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.region.as_str(), "eu-west-1");
    assert_eq!(
        config.regional_api_url.as_str(),
        "https://eu-west-1.api.aex.test"
    );
    assert_eq!(config.limits().json_body_bytes, 65_536);
    assert_eq!(config.limits().query_page_items, 100);
}

#[test]
fn the_listener_binding_is_read_from_the_environment() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.port, 8_080);
    assert_eq!(config.drain_deadline_ms, 25_000);
    assert!(
        config.drain_deadline_ms < 30_000,
        "the drain deadline must expire before the ECS stop timeout sends SIGKILL"
    );
}

#[test]
fn a_port_outside_the_tcp_range_refuses_the_process() {
    for value in ["0", "65536", "not-a-port"] {
        let mut vars = complete();
        vars.insert(config::SESSION_PORT, value.to_owned());
        assert!(
            matches!(
                read(&vars),
                Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::SESSION_PORT
            ),
            "port `{value}` was admitted"
        );
    }
}

#[test]
fn a_drain_deadline_outside_the_admitted_window_refuses_the_process() {
    for value in ["999", "600001"] {
        let mut vars = complete();
        vars.insert(config::SESSION_DRAIN_DEADLINE_MS, value.to_owned());
        assert!(
            matches!(
                read(&vars),
                Err(RegionalHttpConfigError::Invalid { name, .. })
                    if name == config::SESSION_DRAIN_DEADLINE_MS
            ),
            "drain deadline `{value}` was admitted"
        );
    }
}

#[test]
fn a_non_https_regional_api_url_is_refused() {
    let mut vars = complete();
    vars.insert(config::REGIONAL_API_URL, "http://localhost".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::REGIONAL_API_URL
    ));
}

#[test]
fn every_required_variable_is_required() {
    let vars = complete();
    assert_eq!(
        vars.len(),
        config::REQUIRED.len(),
        "the fixture covers exactly the declared variable set"
    );
    for name in config::REQUIRED {
        let mut missing = vars.clone();
        missing.remove(name);
        let error = read(&missing).expect_err("a missing variable refuses the process");
        assert!(
            matches!(error, RegionalHttpConfigError::Missing { name: reported } if reported == name),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn the_finite_api_cannot_be_bound_to_a_queue_or_the_secret_key() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = complete();
        vars.insert(name, "bound".to_owned());
        let error = read(&vars).expect_err("a forbidden binding refuses the process");
        assert!(
            matches!(
                error,
                RegionalHttpConfigError::Forbidden { name: reported, deployable, .. }
                    if reported == name && deployable == config::DEPLOYABLE
            ),
            "binding {name} reported {error:?}"
        );
    }
}

#[test]
fn a_bucket_owned_by_another_account_refuses_the_process() {
    let mut vars = complete();
    vars.insert(config::CONTENT_BUCKET_OWNER, "999999999999".to_owned());
    let error = read(&vars).expect_err("a cross-account bucket owner is refused");
    assert!(
        matches!(error, RegionalHttpConfigError::Invalid { name, .. } if name == config::CONTENT_BUCKET_OWNER),
        "{error:?}"
    );
}

#[test]
fn a_page_bound_outside_the_registry_range_refuses_the_process() {
    let mut vars = complete();
    vars.insert(config::MAX_PAGE_ITEMS, "1001".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::MAX_PAGE_ITEMS
    ));
}

#[test]
fn the_finite_api_owns_every_regional_unary_route_except_the_peers() {
    let owned = RouteOwner::SessionApi.routes();
    assert!(!owned.is_empty(), "the finite API owns no generated routes");
    for id in &owned {
        let descriptor = route(*id);
        assert_eq!(descriptor.plane, Plane::Regional, "`{id}`");
        assert_eq!(
            descriptor.transport,
            TransportKind::Unary,
            "`{id}` is not a frame stream"
        );
    }
    // The plaintext-bearing half of the two split fragments belongs to the
    // secret edge, and this deployable must not claim it.
    for id in [
        RouteId::SecretPut,
        RouteId::SecretDelete,
        RouteId::SecretRevoke,
        RouteId::ProviderCredentialRegister,
    ] {
        assert_eq!(route_owner(id), Some(RouteOwner::SecretApi), "`{id}`");
    }
}

#[test]
fn the_owned_set_is_drawn_from_the_groups_and_not_from_a_second_list() {
    let from_groups: Vec<RouteId> = RouteGroup::ALL
        .iter()
        .flat_map(|group| RouteOwner::SessionApi.routes_in(*group))
        .collect();
    let mut expected = from_groups;
    expected.sort_unstable_by_key(|id| *id as usize);
    assert_eq!(
        expected,
        RouteOwner::SessionApi.routes(),
        "iterating the groups reproduces the owned set exactly"
    );
}
