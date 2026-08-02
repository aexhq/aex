//! What the reconciler is allowed to do, asserted against the allowlist.

use std::path::Path;

/// The committed declarative grant allowlist.
fn grants() -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central/grants.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("`{}` reads: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("`{}` parses: {error}", path.display()))
}

/// The table entries `role` is granted.
fn tables(role: &str) -> Vec<String> {
    grants()
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
fn the_reconciler_can_never_mutate_journal_history() {
    for entry in tables("aex_finance_reconcile") {
        let Some((relation, privileges)) = entry.split_once(':') else {
            panic!("`{entry}` is not a `<relation>:<privileges>` entry");
        };
        if relation.starts_with("finance.journal") {
            for forbidden in ["UPDATE", "DELETE"] {
                assert!(
                    !privileges.contains(forbidden),
                    "F-30: the sweeps report and fence; `{relation}` grants {forbidden}"
                );
            }
        }
    }
}

#[test]
fn the_reconciler_can_resolve_an_effect_but_not_open_one() {
    let entries = tables("aex_finance_reconcile");
    let effect = entries
        .iter()
        .find(|entry| entry.starts_with("finance.provider_effect:"))
        .expect("the reconciler reaches provider effects");
    assert!(
        effect.contains("UPDATE"),
        "an unknown effect must be resolvable"
    );
    assert!(
        !effect.contains("INSERT"),
        "only the request path opens an effect; a sweep never mints one"
    );
}

#[test]
fn the_sweeps_never_hold_a_write_on_the_usage_inbox() {
    for entry in tables("aex_finance_reconcile") {
        if entry.starts_with("finance.usage_inbox:") {
            assert!(
                !entry.contains("INSERT"),
                "settling usage is the settlement worker's duty, not the sweeps'"
            );
        }
    }
}
