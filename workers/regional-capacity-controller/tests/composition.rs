//! Fail-closed composition contract for the capacity controller.

use std::collections::BTreeMap;

use regional_capacity_controller::{Config, ConfigError, keys};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (keys::PLANE, "dev".to_owned()),
        (keys::REGION, "eu-west-1".to_owned()),
        (keys::RELEASE_DIGEST, format!("sha256:{}", "a".repeat(64))),
        (keys::AUTHORITY_TABLE, "dev-capacity-authority".to_owned()),
        (keys::PROJECTION_TABLE, "dev-authz-projection".to_owned()),
    ])
}

#[test]
fn every_binding_is_required_and_tables_must_be_distinct() {
    for key in keys::ALL {
        let mut vars = complete();
        vars.remove(key);
        assert_eq!(
            Config::from_lookup(|name| vars.get(name).cloned()),
            Err(ConfigError::Missing(key))
        );
    }
    let mut vars = complete();
    vars.insert(keys::PROJECTION_TABLE, "dev-capacity-authority".to_owned());
    assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_err());
}

#[test]
fn plane_region_and_release_identity_are_closed() {
    for (key, value) in [
        (keys::PLANE, "stage"),
        (keys::REGION, "mars-1"),
        (keys::RELEASE_DIGEST, "latest"),
    ] {
        let mut vars = complete();
        vars.insert(key, value.to_owned());
        assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_err());
    }
}
