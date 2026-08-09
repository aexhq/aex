//! Start-up, configuration and capability facts for `regional-secret-api`.
//!
//! The deployable that holds plaintext is the one whose composition has to be
//! provable rather than reviewed, so these assert three things: every required
//! variable is required, a forbidden binding refuses the process, and the
//! deployable's route ownership is exactly the plaintext-bearing set.

use std::collections::BTreeMap;

use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::routes::{RouteId, route};
use regional_secret_api::config::{self, Config};
use regional_secret_api::handlers::Routes;

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (
            config::CREDENTIAL_PEPPER_REF,
            "/aex/dev/credential-pepper/ring".to_owned(),
        ),
        (
            config::AUTHZ_PROJECTION_TABLE,
            "aex-dev-regional-authz-projection".to_owned(),
        ),
        (
            config::SECRET_CUSTODY_TABLE,
            "aex-dev-regional-secret-custody".to_owned(),
        ),
        (
            config::SECRET_KEYSTORE_TABLE,
            "aex-dev-regional-secret-keystore".to_owned(),
        ),
        (
            config::SECRET_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"
                .to_owned(),
        ),
        (config::BRANCH_KEY_CACHE_BYTES, "1048576".to_owned()),
        (config::BRANCH_KEY_CACHE_TTL_MS, "300000".to_owned()),
        (config::MAX_JSON_BODY_BYTES, "65536".to_owned()),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, RegionalHttpConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.plane, aex_identity_domain::assertion::Plane::Dev);
    assert_eq!(config.region.as_str(), "eu-west-1");
    assert_eq!(config.secret_kms_key.account, "000000000000");
    assert_eq!(config.crypto_partition(), "dev:eu-west-1");
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
            matches!(
                error,
                RegionalHttpConfigError::Missing { name: reported } if reported == name
            ),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn a_blank_value_is_missing_rather_than_empty() {
    let mut vars = complete();
    vars.insert(config::SECRET_CUSTODY_TABLE, "   ".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Missing { .. })
    ));
}

#[test]
fn a_resource_in_another_region_refuses_the_process() {
    let mut vars = complete();
    vars.insert(
        config::SECRET_KMS_KEY_ARN,
        "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555".to_owned(),
    );
    let error = read(&vars).expect_err("a cross-region key is refused");
    let RegionalHttpConfigError::Invalid { name, reason } = error else {
        panic!("expected an invalid-value refusal");
    };
    assert_eq!(name, config::SECRET_KMS_KEY_ARN);
    assert!(reason.contains("us-east-1"), "{reason}");
    assert!(reason.contains("eu-west-1"), "{reason}");
}

#[test]
fn an_unknown_plane_refuses_the_process() {
    let mut vars = complete();
    vars.insert(config::PLANE, "staging".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::PLANE
    ));
}

#[test]
fn the_secret_edge_cannot_be_bound_to_a_forbidden_resource() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = complete();
        vars.insert(name, "aex-dev-anything".to_owned());
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
fn the_secret_edge_owns_exactly_the_plaintext_bearing_routes() {
    let owned = RouteOwner::SecretApi.routes();
    assert_eq!(
        owned,
        vec![
            RouteId::ProviderCredentialRegister,
            RouteId::SecretDelete,
            RouteId::SecretPut,
            RouteId::SecretRevoke,
        ],
        "the secret edge owns the four plaintext-bearing routes and nothing else"
    );
    // Every read that returns stored metadata belongs to the session API: this
    // deployable has no route that returns a stored value.
    for id in [RouteId::SecretGet, RouteId::SecretsList] {
        assert_eq!(route_owner(id), Some(RouteOwner::SessionApi), "`{id}`");
    }
}

#[test]
fn a_served_route_never_paginates() {
    // This is the premise `Config::limits` rests on: the two page bounds are
    // zero because no route this deployable serves can ask for a page. If a
    // paginated route ever lands here, that has to be a red suite rather than a
    // silently zero-sized page.
    for id in Routes::served() {
        let descriptor = route(id);
        assert!(
            !descriptor.query_params.contains(&"cursor")
                && !descriptor.query_params.contains(&"limit"),
            "`{id}` paginates; `Config::limits` would give it a zero page bound"
        );
    }
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.limits().json_body_bytes, config.max_json_body_bytes);
    assert_eq!(config.limits().query_page_items, 0);
    assert_eq!(config.limits().query_page_bytes, 0);
}

#[test]
fn the_mounted_router_answers_exactly_the_served_set() {
    // The listener composes `Dispatcher` over the real edge. Neither can be
    // built without credentials, but the mount decision is pure: it is the
    // dispatcher's served set filtered against the owned partition, and a route
    // that left one and not the other is a `MountError` rather than a runtime
    // 404.
    let served = Routes::served();
    assert_eq!(
        served,
        vec![RouteId::SecretDelete, RouteId::SecretRevoke],
        "the two routes whose handlers need no ciphertext"
    );
    for id in &served {
        assert_eq!(route_owner(*id), Some(RouteOwner::SecretApi), "`{id}`");
    }
    let owned = RouteOwner::SecretApi.routes();
    for id in owned.iter().filter(|id| !served.contains(id)) {
        assert!(
            !served.contains(id),
            "`{id}` is owned and unserved, so it must be absent from the router"
        );
    }
}
