//! Structural proof that this adapter cannot write a sibling authority.
//!
//! `U-19` requires three independent controls. Two of them are testable here and
//! are tested here rather than reviewed:
//!
//! 1. **Link graph** — this crate does not depend on either sibling adapter, so
//!    no sibling expression builder is reachable from it at all.
//! 2. **Source conformance** — exactly one table binding, one receipt-queue
//!    binding, and no string naming a sibling authority's table or queue.
//!
//! The third control is the `DynamoDB` resource-based policy with a
//! `NotPrincipal` deny, which needs a live account and is asserted by the live
//! companion. Identity policy alone is one review mistake away from failing
//! open, which is why none of the three stands on its own.

use std::path::{Path, PathBuf};

/// This crate's own directory.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every Rust source file in this crate.
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, found: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("the crate's own source tree is readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                found.push((path, text));
            }
        }
    }
    let mut found = Vec::new();
    walk(&crate_dir().join("src"), &mut found);
    assert!(!found.is_empty(), "the crate must have sources to scan");
    found
}

/// The two sibling adapters this crate must not link.
const SIBLING_CRATES: [&str; 2] = ["aex-usage-compute-aws", "aex-usage-transfer-aws"];

/// The two sibling authorities this crate must not name.
const SIBLING_CATEGORIES: [&str; 2] = ["compute", "transfer"];

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
fn exactly_one_table_binding_exists() {
    let declarations: usize = sources()
        .iter()
        .map(|(_, text)| text.matches("pub const TABLE_ENV").count())
        .sum();
    assert_eq!(
        declarations, 1,
        "one adapter addresses one table; a second binding is how a category \
         boundary erodes quietly"
    );
}

#[test]
fn exactly_one_receipt_queue_binding_exists() {
    let declarations: usize = sources()
        .iter()
        .map(|(_, text)| text.matches("pub const RECEIPT_QUEUE_ENV").count())
        .sum();
    assert_eq!(declarations, 1);
}

#[test]
fn no_source_names_a_sibling_table_or_queue() {
    for (path, text) in sources() {
        // Test modules legitimately name the sibling categories to prove they
        // are refused, so only the non-test half of each file is scanned.
        let body = text.split("#[cfg(test)]").next().unwrap_or_default();
        for sibling in SIBLING_CATEGORIES {
            for forbidden in [
                format!("usage-{sibling}-authority"),
                format!("AEX_USAGE_{}_TABLE", sibling.to_uppercase()),
                format!("AEX_USAGE_RECEIPT_{}_QUEUE", sibling.to_uppercase()),
            ] {
                assert!(
                    !body.contains(&forbidden),
                    "`{}` names `{forbidden}`, which belongs to a sibling authority",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn the_category_constant_is_the_single_binding() {
    // One declaration, and it is the category this crate is named for.
    let declarations: usize = sources()
        .iter()
        .map(|(_, text)| text.matches("pub const CATEGORY").count())
        .sum();
    assert_eq!(declarations, 1);
    assert_eq!(
        aex_usage_storage_aws::CATEGORY.id(),
        "storage",
        "the crate's category must match its name"
    );
}
