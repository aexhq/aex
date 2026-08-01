//! Proof that the query adapter cannot write.
//!
//! "Read-only" has to be a test result, not a convention (`U-20`). Four controls
//! are required and three of them are provable here without an account:
//!
//! 1. **Source conformance** — no write operation name appears anywhere in this
//!    crate's sources, in either the SDK's `snake_case` or its `PascalCase` form.
//! 2. **Link graph** — this crate does not depend on any of the three authority
//!    adapters, so no write expression builder is even reachable from it.
//! 3. **IAM shape** — the generation definition for `usage-query-projection`
//!    grants the reading role `GetItem` and `Query` and nothing else, and grants
//!    a write action to no role outside the three usage workers.
//!
//! The fourth is a live `AccessDeniedException` assertion against a real table,
//! which needs a deployed plane and lives in `tests/live/aex-live-usage-*-worker`.

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

/// The `usage-query-projection` generation definition.
fn projection_definition() -> serde_json::Value {
    let path = crate_dir()
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name> always has two ancestors")
        .join("migrations/regional/tables/usage-query-projection.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the definition is valid JSON")
}

/// Every action that can change a row.
const WRITE_ACTIONS: [&str; 6] = [
    "dynamodb:PutItem",
    "dynamodb:UpdateItem",
    "dynamodb:DeleteItem",
    "dynamodb:BatchWriteItem",
    "dynamodb:TransactWriteItems",
    "dynamodb:ExecuteStatement",
];

/// The only roles that may write the projection.
const WRITERS: [&str; 3] = [
    "usage-storage-worker",
    "usage-compute-worker",
    "usage-transfer-worker",
];

#[test]
fn the_reading_role_holds_read_actions_and_nothing_else() {
    let definition = projection_definition();
    let grants = definition["iam"]
        .as_array()
        .expect("the definition declares IAM grants");
    let reader = grants
        .iter()
        .find(|grant| grant["role"] == "regional-session-api")
        .expect("the customer read path holds a grant");

    let actions: Vec<&str> = reader["actions"]
        .as_array()
        .expect("actions are a list")
        .iter()
        .map(|action| action.as_str().expect("an action is a string"))
        .collect();
    assert_eq!(
        actions,
        vec!["dynamodb:GetItem", "dynamodb:Query"],
        "the API role reads the projection and does nothing else to it"
    );
    assert_eq!(
        reader["resources"]
            .as_array()
            .expect("resources are a list"),
        &vec![serde_json::Value::from("table")],
        "no index grant either: the projection has no index to read"
    );
}

#[test]
fn only_the_three_usage_workers_may_write_the_projection() {
    // The projection is shared by three categories, so the IAM shape is what
    // keeps a fourth principal out. A cross-category write here is repairable
    // by a rebuild; a write by anything else is not accounted for at all.
    for grant in projection_definition()["iam"]
        .as_array()
        .expect("the definition declares IAM grants")
    {
        let role = grant["role"].as_str().expect("a role is a string");
        for action in grant["actions"].as_array().expect("actions are a list") {
            let action = action.as_str().expect("an action is a string");
            if WRITE_ACTIONS.contains(&action) {
                assert!(
                    WRITERS.contains(&role),
                    "`{role}` holds `{action}` on usage-query-projection"
                );
            }
        }
    }
}

#[test]
fn the_projection_exposes_no_change_feed_and_no_index() {
    // A stream would let a write fan out into another system before the
    // authority had settled it, and an index would widen what one customer read
    // can observe. The projection needs neither.
    let definition = projection_definition();
    assert_eq!(
        definition["stream"]["enabled"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(definition["stream"]["viewType"], "NONE");
    assert_eq!(
        definition["globalSecondaryIndexes"]
            .as_array()
            .expect("indexes are a list")
            .len(),
        0
    );
}

#[test]
fn only_detail_rows_expire_and_the_authorities_expire_nothing() {
    let definition = projection_definition();
    assert_eq!(
        definition["timeToLive"]["enabled"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        definition["timeToLive"]["appliesTo"]
            .as_array()
            .expect("appliesTo is a list"),
        &vec![serde_json::Value::from("usage_detail")],
        "aggregates and the coverage vector must never expire underneath a read"
    );

    for authority in [
        "usage-storage-authority",
        "usage-compute-authority",
        "usage-transfer-authority",
    ] {
        let path = crate_dir()
            .parent()
            .and_then(Path::parent)
            .expect("crates/<name> always has two ancestors")
            .join(format!("migrations/regional/tables/{authority}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let definition: serde_json::Value =
            serde_json::from_str(&text).expect("the definition is valid JSON");
        assert_eq!(
            definition["timeToLive"]["enabled"],
            serde_json::Value::Bool(false),
            "every row in `{authority}` is money evidence and must not expire"
        );
    }
}
