//! Per-mode start-up and capability facts for `content-lifecycle-worker`.
//!
//! The point of the four-role split is that only one role can ever delete an
//! object. These assert that in both directions: `delete` cannot start without
//! the capability, and no other role can start holding it.

use std::collections::BTreeMap;

use aex_regional_http::capability::{Capability as _, ContentObjectDelete};
use aex_regional_http::config::ConfigError;
use content_lifecycle_worker::config::{self, Config, Mode};

fn base(mode: &str) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::MODE, mode.to_owned()),
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (config::CONTENT_TABLE, "aex-dev-regional-content".to_owned()),
        (
            config::REGISTRY_TABLE,
            "aex-dev-regional-registry".to_owned(),
        ),
        (config::WORK_TABLE, "aex-dev-regional-work".to_owned()),
        (
            config::CONTENT_BUCKET,
            "aex-dev-eu-west-1-content".to_owned(),
        ),
        (config::CONTENT_BUCKET_OWNER, "000000000000".to_owned()),
        (
            config::DENIAL_PROJECTION_TABLE,
            "aex-dev-deletion-denial".to_owned(),
        ),
        (config::GC_STAGE_GRACE_HOURS, "24".to_owned()),
        (config::UPLOAD_GRACE_HOURS, "24".to_owned()),
        (
            config::DECLARED_CAPABILITIES,
            config::NO_CAPABILITIES.to_owned(),
        ),
    ])
}

fn expiry() -> BTreeMap<&'static str, String> {
    base("expiry")
}

fn marksweep() -> BTreeMap<&'static str, String> {
    let mut vars = base("marksweep");
    vars.insert(config::MARK_PAGE_ITEMS, "500".to_owned());
    vars.insert(config::SWEEP_PAGE_ITEMS, "500".to_owned());
    vars
}

fn delete() -> BTreeMap<&'static str, String> {
    let mut vars = base("delete");
    vars.insert(
        config::DECLARED_CAPABILITIES,
        ContentObjectDelete::ID.to_owned(),
    );
    vars.insert(
        config::CONTENT_QUEUE_URL,
        "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-content-deletion".to_owned(),
    );
    vars.insert(
        config::CONTENT_DLQ_URL,
        "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-content-deletion-dlq".to_owned(),
    );
    vars
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn each_mode_accepts_its_own_complete_environment() {
    assert_eq!(read(&expiry()).expect("expiry").mode, Mode::Expiry);
    assert_eq!(read(&marksweep()).expect("marksweep").mode, Mode::MarkSweep);
    let deleting = read(&delete()).expect("delete");
    assert_eq!(deleting.mode, Mode::Delete);
    assert!(deleting.content_queue_url.is_some());
}

#[test]
fn every_common_variable_is_required_in_every_mode() {
    for vars in [expiry(), marksweep(), delete()] {
        for name in config::REQUIRED_COMMON {
            let mut missing = vars.clone();
            missing.remove(name);
            let error = read(&missing).expect_err("a missing variable refuses the process");
            assert!(
                matches!(error, ConfigError::Missing { name: reported } if reported == name)
                    || matches!(error, ConfigError::Invalid { name: reported, .. } if reported == name),
                "removing {name} reported {error:?}"
            );
        }
    }
}

#[test]
fn delete_mode_cannot_start_without_the_object_delete_capability() {
    let mut vars = delete();
    vars.insert(
        config::DECLARED_CAPABILITIES,
        config::NO_CAPABILITIES.to_owned(),
    );
    let error = read(&vars).expect_err("delete without the capability is refused");
    let ConfigError::Invalid { name, reason } = error else {
        panic!("expected an invalid-value refusal, got {error:?}");
    };
    assert_eq!(name, config::DECLARED_CAPABILITIES);
    assert!(reason.contains(ContentObjectDelete::ID), "{reason}");
}

#[test]
fn no_other_mode_may_hold_the_object_delete_capability() {
    for mode in ["expiry", "reconcile", "marksweep"] {
        let mut vars = base(mode);
        if mode == "marksweep" {
            vars.insert(config::MARK_PAGE_ITEMS, "500".to_owned());
            vars.insert(config::SWEEP_PAGE_ITEMS, "500".to_owned());
        }
        vars.insert(
            config::DECLARED_CAPABILITIES,
            ContentObjectDelete::ID.to_owned(),
        );
        let error = read(&vars).expect_err("a non-delete role holding the capability is refused");
        assert!(
            matches!(error, ConfigError::Forbidden { name, .. } if name == config::DECLARED_CAPABILITIES),
            "{mode} reported {error:?}"
        );
    }
}

#[test]
fn delete_mode_requires_its_queue_and_the_others_do_not() {
    let mut vars = delete();
    vars.remove(config::CONTENT_QUEUE_URL);
    assert!(matches!(
        read(&vars),
        Err(ConfigError::Missing { name }) if name == config::CONTENT_QUEUE_URL
    ));
    // A scheduled role never needs the queue at all.
    assert!(read(&expiry()).expect("expiry").content_queue_url.is_none());
}

#[test]
fn marksweep_requires_its_page_bounds() {
    let mut vars = marksweep();
    vars.remove(config::MARK_PAGE_ITEMS);
    assert!(matches!(
        read(&vars),
        Err(ConfigError::Missing { name }) if name == config::MARK_PAGE_ITEMS
    ));
}

#[test]
fn an_unknown_mode_refuses_the_process() {
    let mut vars = expiry();
    vars.insert(config::MODE, "sweepmark".to_owned());
    assert!(matches!(
        read(&vars),
        Err(ConfigError::Invalid { name, .. }) if name == config::MODE
    ));
}
