//! Start-up denial facts for `regional-secret-key-admin`.
//!
//! Startup denial is this deployable's whole security posture, so it is what the
//! composition test asserts: a wrong keystore, a cross-region key, an
//! unattested run and every product-table binding all refuse the process before
//! any AWS call.

use std::collections::BTreeMap;

use aex_regional_http::config::ConfigError;
use regional_secret_key_admin::config::{self, Config};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (
            config::SECRET_KEYSTORE_TABLE,
            "aex-dev-regional-secret-keystore".to_owned(),
        ),
        (
            config::KEYSTORE_LOGICAL_NAME,
            "regional-secret-keystore".to_owned(),
        ),
        (
            config::SECRET_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"
                .to_owned(),
        ),
        (
            config::ATTESTATION_OPERATION_ID,
            "op_0e5t8mmr8g0e5t8mmr8g0e5t8m".to_owned(),
        ),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("the complete environment is accepted");
    assert_eq!(config.keystore_logical_name, "regional-secret-keystore");
    assert_eq!(config.secret_kms_key.region, "eu-west-1");
}

#[test]
fn every_required_variable_is_required() {
    let vars = complete();
    assert_eq!(vars.len(), config::REQUIRED.len());
    for name in config::REQUIRED {
        let mut missing = vars.clone();
        missing.remove(name);
        let error = read(&missing).expect_err("a missing variable refuses the process");
        assert!(
            matches!(error, ConfigError::Missing { name: reported } if reported == name),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn a_table_that_is_not_the_configured_keystore_refuses_the_process() {
    let mut vars = complete();
    vars.insert(
        config::SECRET_KEYSTORE_TABLE,
        "aex-dev-regional-secret-custody".to_owned(),
    );
    let error = read(&vars).expect_err("a non-keystore table is refused");
    assert!(
        matches!(error, ConfigError::Invalid { name, .. } if name == config::SECRET_KEYSTORE_TABLE),
        "{error:?}"
    );
}

#[test]
fn a_key_in_another_region_refuses_the_process() {
    let mut vars = complete();
    vars.insert(
        config::SECRET_KMS_KEY_ARN,
        "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555".to_owned(),
    );
    assert!(matches!(
        read(&vars),
        Err(ConfigError::Invalid { name, .. }) if name == config::SECRET_KMS_KEY_ARN
    ));
}

#[test]
fn an_unattested_run_refuses_the_process() {
    let mut vars = complete();
    vars.insert(config::ATTESTATION_OPERATION_ID, "not-an-op".to_owned());
    assert!(matches!(
        read(&vars),
        Err(ConfigError::Invalid { name, .. }) if name == config::ATTESTATION_OPERATION_ID
    ));
}

#[test]
fn every_product_table_binding_refuses_the_process() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = complete();
        vars.insert(name, "aex-dev-anything".to_owned());
        let error = read(&vars).expect_err("a product binding refuses the key admin");
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
