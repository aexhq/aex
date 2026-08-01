//! Proof that the query adapter cannot write.
//!
//! "Read-only" has to be a test result, not a convention (`U-20`). Four controls
//! are required and two of them are provable here without an account:
//!
//! 1. **Source conformance** — no write operation name appears anywhere in this
//!    crate's sources, in either the SDK's `snake_case` or its `PascalCase` form.
//! 2. **Link graph** — this crate does not depend on any of the three authority
//!    adapters, so no write expression builder is even reachable from it.
//!
//! The other two are the IAM policy shape and a live `AccessDeniedException`
//! assertion, both of which need a deployed plane and live in
//! `tests/live/aex-live-usage-*-worker`.

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

/// Every mutating `DynamoDB` operation, in both spellings the SDK uses.
const WRITE_OPERATIONS: [&str; 12] = [
    "put_item",
    "update_item",
    "delete_item",
    "batch_write_item",
    "transact_write_item",
    "execute_statement",
    "PutItem",
    "UpdateItem",
    "DeleteItem",
    "BatchWriteItem",
    "TransactWriteItems",
    "ExecuteStatement",
];

#[test]
fn no_source_names_a_write_operation() {
    for (path, text) in sources() {
        for operation in WRITE_OPERATIONS {
            assert!(
                !text.contains(operation),
                "`{}` names `{operation}`; the query adapter is read-only and \
                 that has to be a test result rather than a convention",
                path.display()
            );
        }
    }
}

#[test]
fn this_crate_does_not_link_any_authority_adapter() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml"))
        .expect("the crate's own manifest is readable");
    for authority in [
        "aex-usage-storage-aws",
        "aex-usage-compute-aws",
        "aex-usage-transfer-aws",
    ] {
        assert!(
            !manifest.contains(authority),
            "`{authority}` must not be reachable: an unreachable write expression \
             builder cannot be called by mistake"
        );
    }
}

#[test]
fn the_projection_is_generation_keyed_so_a_rebuild_is_a_normal_operation() {
    use aex_usage_domain::meter::PublicCategory;
    use aex_usage_domain::wire_pending::WorkspaceId;
    use aex_usage_query_aws::expressions::{Generation, ProjectionKeys};

    let workspace = WorkspaceId::parse("ws-1").expect("workspace");
    let keys = ProjectionKeys;

    // Old and new generations occupy disjoint key space, so a rebuild never
    // partially overwrites the generation still being read.
    let live = keys
        .coverage(Generation::FIRST, &workspace, PublicCategory::Storage)
        .expect("builds");
    let rebuilt = keys
        .coverage(
            Generation::FIRST.next().expect("advances"),
            &workspace,
            PublicCategory::Storage,
        )
        .expect("builds");
    assert_ne!(live.pk, rebuilt.pk);

    // The pointer is the only thing a cutover touches.
    let pointer = keys.generation_pointer();
    assert_ne!(pointer.pk, live.pk);
    assert_ne!(pointer.pk, rebuilt.pk);
}
