//! Structural proof that this adapter cannot write a sibling authority.
//!
//! `U-19` requires independent controls. The local checks cover the link graph,
//! the typed category binding, and the migration/IAM shape; the live companion
//! covers the deployed resource policy.
//!
//! 1. **Link graph** — this crate does not depend on either sibling adapter, so
//!    no sibling expression builder is reachable from it at all.
//!
//! The deployed control is the `DynamoDB` resource-based policy with a
//! `NotPrincipal` deny, which needs a live account and is asserted by the live
//! companion. Identity policy alone is one review mistake away from failing
//! open, which is why none of the three stands on its own.

use aex_usage_domain::keys::ItemType;
use std::path::{Path, PathBuf};

/// This crate's own directory.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The two sibling adapters this crate must not link.
const SIBLING_CRATES: [&str; 2] = ["aex-usage-storage-dynamodb", "aex-usage-compute-dynamodb"];

#[test]
fn this_adapter_does_not_link_a_sibling_adapter() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml"))
        .expect("the crate's own manifest is readable");
    for sibling in SIBLING_CRATES {
        assert!(
            !manifest.contains(sibling),
            "`{sibling}` must not be reachable from this crate: a worker that \
             cannot link a sibling's expression builder cannot write its table \
             even with the wrong credentials"
        );
    }
}

#[test]
fn the_category_binding_matches_the_adapter() {
    assert_eq!(
        aex_usage_transfer_dynamodb::CATEGORY.id(),
        "transfer",
        "the crate's category must match its name"
    );
}

/// This authority's generation definition.
fn definition() -> serde_json::Value {
    let path = crate_dir()
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name> always has two ancestors")
        .join("migrations/regional/tables/usage-transfer-authority.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the definition is valid JSON")
}

#[test]
fn the_table_definition_declares_exactly_the_item_types_this_category_holds() {
    // The generation definition and the key grammar are two statements of one
    // vocabulary. A row this adapter can mint but the table does not declare
    // would decode as corrupt on the way back out.
    let declared: Vec<String> = definition()["itemTypes"]
        .as_array()
        .expect("itemTypes is a list")
        .iter()
        .map(|value| value.as_str().expect("an item type is a string").to_owned())
        .collect();

    let mut expected: Vec<String> = ItemType::ALL
        .into_iter()
        .filter(|item| item.allowed_in(aex_usage_transfer_dynamodb::CATEGORY))
        .map(|item| item.id().to_owned())
        .collect();
    expected.sort();
    let mut declared_sorted = declared;
    declared_sorted.sort();
    assert_eq!(
        declared_sorted, expected,
        "the table vocabulary and the key grammar must be one vocabulary"
    );
}

#[test]
fn this_authority_expires_nothing_and_streams_only_its_own_worker() {
    let definition = definition();
    assert_eq!(
        definition["timeToLive"]["enabled"],
        serde_json::Value::Bool(false),
        "every row here is money evidence"
    );
    assert_eq!(
        definition["stream"]["enabled"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(definition["stream"]["viewType"], "NEW_IMAGE");
    assert_eq!(
        definition["stream"]["consumers"]
            .as_array()
            .expect("consumers are a list"),
        &vec![serde_json::Value::from("usage-transfer-worker")],
        "one authority, one stream, one worker"
    );
}

#[test]
fn no_sibling_worker_holds_a_grant_on_this_authority() {
    // The identity policy is the second of the three isolation controls, and it
    // is only worth anything if no sibling appears in it at all.
    for grant in definition()["iam"]
        .as_array()
        .expect("the definition declares IAM grants")
    {
        let role = grant["role"].as_str().expect("a role is a string");
        for sibling in ["usage-storage-worker", "usage-compute-worker"] {
            assert_ne!(
                role, sibling,
                "a sibling must hold nothing on this authority"
            );
        }
    }
}

#[test]
fn both_indexes_project_the_discriminator_they_are_decoded_through() {
    // An INCLUDE projection carries no discriminator of its own, so a row read
    // back through an index without `itemType` is a row decoded as the wrong
    // shape. Projecting it is cheaper than a second decode path.
    let definition = definition();
    let indexes = definition["globalSecondaryIndexes"]
        .as_array()
        .expect("indexes are a list");
    assert_eq!(indexes.len(), 2);
    for index in indexes {
        let attributes: Vec<&str> = index["projection"]["attributes"]
            .as_array()
            .expect("attributes are a list")
            .iter()
            .map(|value| value.as_str().expect("an attribute is a string"))
            .collect();
        assert!(
            attributes.contains(&"itemType"),
            "index `{}` projects no discriminator",
            index["name"]
        );
        assert_eq!(index["projection"]["type"], "INCLUDE");
        assert_eq!(index["sparse"], serde_json::Value::Bool(true));
    }
}
