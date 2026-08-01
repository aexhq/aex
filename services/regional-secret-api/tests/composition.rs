//! Start-up, configuration and capability facts for `regional-secret-api`.
//!
//! The deployable that holds plaintext is the one whose composition has to be
//! provable rather than reviewed, so these assert three things: every required
//! variable is required, a forbidden binding refuses the process, and the
//! deployable's route ownership is exactly the plaintext-bearing set.

use std::collections::BTreeMap;

use aex_regional_http::config::ConfigError;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::routes::RouteId;
use regional_secret_api::config::{self, Config};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
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
        (config::ASSERTION_CACHE_BYTES, "1048576".to_owned()),
        (config::MAX_JSON_BODY_BYTES, "65536".to_owned()),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.plane, "dev");
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
                ConfigError::Missing { name: reported } if reported == name
            ),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn a_blank_value_is_missing_rather_than_empty() {
    let mut vars = complete();
    vars.insert(config::SECRET_CUSTODY_TABLE, "   ".to_owned());
    assert!(matches!(read(&vars), Err(ConfigError::Missing { .. })));
}

#[test]
fn a_resource_in_another_region_refuses_the_process() {
    let mut vars = complete();
    vars.insert(
        config::SECRET_KMS_KEY_ARN,
        "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555".to_owned(),
    );
    let error = read(&vars).expect_err("a cross-region key is refused");
    let ConfigError::Invalid { name, reason } = error else {
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
        Err(ConfigError::Invalid { name, .. }) if name == config::PLANE
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
                ConfigError::Forbidden { name: reported, deployable, .. }
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
