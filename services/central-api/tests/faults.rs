//! `central-api` fail-closed configuration evidence.
//!
//! Two properties, and the second is the one that matters for a merge: an
//! incomplete environment must be refused **by name**. This deployable reads
//! twenty-three variables where the three it replaces read thirteen, eleven and
//! twelve, so "it did not start" without a variable name is a deployment nobody
//! can debug.

use std::collections::BTreeMap;
use std::process::Command;

use central_api::config::{self, CentralApiConfigError, Config};

#[test]
fn the_artifact_refuses_to_start_without_its_exact_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_central-api"))
        .env_clear()
        .output()
        .expect("`central-api` starts");
    assert!(!output.status.success(), "an empty environment is refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to start"),
        "the refusal names itself: {stderr}"
    );
    assert!(
        stderr.contains("AEX_CENTRAL_API_"),
        "the refusal names a variable: {stderr}"
    );
}

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&complete()).expect("a complete environment");
    assert_eq!(config.http.service, config::DEPLOYABLE);
    assert_eq!(config.port, 8080);
    assert_eq!(config.api_urls.len(), 1);
}

#[test]
fn the_declared_variable_list_is_exactly_what_the_reader_reads() {
    // The list is what a refusal prints, so a variable the reader requires and
    // the list omits is a variable an operator is never told about — and a
    // variable the list names and the reader ignores is one an operator sets
    // for nothing.
    for name in config::ALL {
        let mut vars = complete();
        vars.remove(name);
        assert!(
            read(&vars).is_err(),
            "`{name}` is listed as required and removing it was accepted"
        );
    }
    for name in complete().keys() {
        assert!(
            config::ALL.contains(name),
            "`{name}` is supplied by the fixture and is not in the declared list"
        );
    }
    assert_eq!(
        config::ALL.len(),
        complete().len(),
        "the fixture and the declared list must describe the same environment"
    );
}

#[test]
fn a_drain_deadline_that_could_not_fire_before_sigkill_is_refused() {
    let mut vars = complete();
    vars.insert(
        config::DRAIN_DEADLINE_MS,
        (aex_regional_http::drain::MAX_DRAIN_DEADLINE_MS + 1).to_string(),
    );
    assert!(
        read(&vars).is_err(),
        "a deadline above the ceiling is a deadline that never fires; the task is \
         killed mid-transaction instead of exiting on its own terms"
    );
    vars.insert(
        config::DRAIN_DEADLINE_MS,
        aex_regional_http::drain::MAX_DRAIN_DEADLINE_MS.to_string(),
    );
    assert!(read(&vars).is_ok(), "the ceiling itself is admissible");
}

#[test]
fn a_context_window_wider_than_the_shared_ceiling_is_refused() {
    let mut vars = complete();
    vars.insert(
        config::CONTEXT_LIFETIME_MS,
        (aex_central_http::authorizer::MAX_CONTEXT_LIFETIME_MS + 1).to_string(),
    );
    assert!(read(&vars).is_err());
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, CentralApiConfigError> {
    Config::from_lookup(|name| vars.get(name).cloned())
}

/// A complete environment, in the shape a deployment root supplies.
fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::ACCOUNT_ID, "000000000000".to_owned()),
        (
            config::API_KEY_PEPPER_SECRET_ID,
            "aex/dev/api-key-pepper/current".to_owned(),
        ),
        (
            config::API_URLS,
            "eu-west-1=https://api.eu-west-1.aex.dev".to_owned(),
        ),
        (
            config::AURORA_CLUSTER_ARN,
            "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
        ),
        (
            config::AURORA_SECRET_ARN,
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-central".to_owned(),
        ),
        (config::CONTEXT_LIFETIME_MS, "30000".to_owned()),
        (
            config::CURSOR_SECRET_ID,
            "aex/dev/cursor-secret/current".to_owned(),
        ),
        (config::DATABASE, "aex".to_owned()),
        (config::DRAIN_DEADLINE_MS, "20000".to_owned()),
        (
            config::IDENTITY_PEPPER_SECRET_ID,
            "aex/dev/identity-pepper/current".to_owned(),
        ),
        (
            config::GOOGLE_OAUTH_SECRET_ID,
            "aex/dev/sign-in/google/current".to_owned(),
        ),
        (
            config::GITHUB_OAUTH_SECRET_ID,
            "aex/dev/sign-in/github/current".to_owned(),
        ),
        (
            config::SIGN_IN_REDIRECT_URI,
            "https://dash.aex.dev/auth/callback".to_owned(),
        ),
        (config::MAX_BODY_BYTES, "65536".to_owned()),
        (config::PAGE_LIMIT, "100".to_owned()),
        (config::PLANE, "dev".to_owned()),
        (config::PORT, "8080".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::REQUEST_DEADLINE_MS, "10000".to_owned()),
        (
            config::STRIPE_COMMAND_EDGE_ARN,
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-stripe-command-edge".to_owned(),
        ),
    ])
}
