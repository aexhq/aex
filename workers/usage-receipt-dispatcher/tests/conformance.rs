//! What the dispatcher is allowed to do, asserted against the allowlist.

use std::path::Path;

use usage_receipt_dispatcher::config::CATEGORIES;

/// The table entries `role` is granted.
fn tables(role: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central/grants.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("`{}` reads: {error}", path.display()));
    let document: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("`{}` parses: {error}", path.display()));
    document
        .get("role")
        .and_then(toml::Value::as_array)
        .expect("grants.toml declares roles")
        .iter()
        .find(|entry| entry.get("name").and_then(toml::Value::as_str) == Some(role))
        .unwrap_or_else(|| panic!("grants.toml declares `{role}`"))
        .get("tables")
        .and_then(toml::Value::as_array)
        .expect("a role declares its tables")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn the_dispatcher_can_neither_create_nor_alter_a_settlement() {
    for entry in tables("aex_receipt_dispatcher") {
        let Some((relation, privileges)) = entry.split_once(':') else {
            panic!("`{entry}` is not a `<relation>:<privileges>` entry");
        };
        if relation != "finance.receipt_outbox" {
            for forbidden in ["INSERT", "UPDATE", "DELETE"] {
                assert!(
                    !privileges.contains(forbidden),
                    "`{relation}` grants {forbidden}; the dispatcher moves receipts only"
                );
            }
        }
    }
}

#[test]
fn the_dispatcher_reaches_the_outbox_and_reaches_it_only_to_retire_a_receipt() {
    let entries = tables("aex_receipt_dispatcher");
    let outbox = entries
        .iter()
        .find(|entry| entry.starts_with("finance.receipt_outbox:"))
        .expect("the dispatcher reaches the outbox");
    assert!(outbox.contains("SELECT"), "it must read the pending page");
    assert!(outbox.contains("UPDATE"), "it must retire a receipt");
    assert!(
        !outbox.contains("INSERT"),
        "a receipt is written by settlement, never by the dispatcher"
    );
    assert!(!outbox.contains("DELETE"), "a receipt is never deleted");
}

#[test]
fn the_three_categories_are_exactly_the_three_regional_receipt_queues() {
    assert_eq!(CATEGORIES, ["compute", "storage", "transfer"]);
    let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
    assert!(
        ddl.contains("receipt_outbox"),
        "the outbox the dispatcher drains is a declared table"
    );
}
