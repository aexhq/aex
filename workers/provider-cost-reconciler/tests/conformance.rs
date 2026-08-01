//! F-26 as a checkable fact: the private COGS family has no path to a customer
//! posting, and the allowlist is what makes that true.

use std::path::Path;

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
fn the_cost_reconciler_reaches_exactly_one_relation() {
    let entries = tables("aex_provider_cost");
    assert_eq!(
        entries,
        vec!["finance.provider_cost_fact:SELECT,INSERT".to_owned()],
        "F-26: the COGS family is the only relation this role can name"
    );
}

#[test]
fn the_cost_reconciler_can_never_reach_a_customer_amount() {
    for entry in tables("aex_provider_cost") {
        for forbidden in [
            "journal_transaction",
            "journal_posting",
            "account_balance",
            "billing_account",
            "reservation",
            "usage_inbox",
            "receipt_outbox",
            "statement",
        ] {
            assert!(
                !entry.contains(forbidden),
                "`{entry}` reaches `{forbidden}`; a cost row must never become a charge"
            );
        }
    }
}

#[test]
fn the_cost_family_is_keyed_on_the_export_row_so_a_duplicate_is_a_no_op() {
    let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
    assert!(
        ddl.contains("PRIMARY KEY (source, source_row_id)"),
        "a duplicate export row must normalise on its own identity"
    );
    assert!(
        ddl.contains("cost_microusd bigint NOT NULL CHECK (cost_microusd >= 0)"),
        "cost is a non-negative integer micro-USD amount"
    );
}
