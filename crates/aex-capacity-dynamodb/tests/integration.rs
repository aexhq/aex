//! Engine-backed cases for the regional capacity authority, against `DynamoDB` Local.
//!
//! This file replaces the crate's `integration` deferral, whose stated reason —
//! that the workspace had no approved `DynamoDB` Local harness — stopped being
//! true when `aex_test_harness::containers` landed.
//!
//! The whole design of this adapter is one claim: **the authority row, its
//! immutable audit row, every member limit row, the bundle payload, the bundle
//! head and the hot edge subset become visible together or not at all**, across
//! *two* physical tables. Nothing short of a real `TransactWriteItems` can
//! evidence that. The unit suite proves the plan is shaped that way; these cases
//! prove the engine commits it that way, and — the half that matters far more —
//! that a refused command leaves **nothing** behind.
//!
//! What only a real engine proves here:
//!
//! - a bootstrap materialises `2 + N + 3` rows across two tables in one commit,
//!   with `N = LimitId::ALL`, which is also where the 100-action ceiling that
//!   caps the registry at 95 limits stops being arithmetic and starts being
//!   observable;
//! - the immutable audit row really is immutable: a revision whose audit row
//!   already exists cancels at `capacity.audit`, and the authority row does not
//!   move;
//! - a cancelled commit leaves the serving projection at the previous revision,
//!   so a request edge reading the edge-limit row alone can never observe a
//!   revision the authority never published.
//!
//! What it does not prove, stated rather than assumed away:
//!
//! - **`TransactionConflict` under real contention.** `release/policy/test-images.toml`
//!   records it as `cannot_prove` for this engine, so the ambiguity resolver's
//!   re-read arm is covered by the unit suite and by the live companion, not
//!   here.
//! - **`aws.iam.denial`.** The `LeadingKeys` condition on the controller's role
//!   is a deployed fact; this engine authorises nothing.

use std::collections::BTreeMap;

use aex_capacity_dynamodb::defaults::canonical_defaults;
use aex_capacity_dynamodb::model::{CapacityCommand, CapacityError};
use aex_capacity_dynamodb::store::{CapacityStore, CapacityStoreError};
use aex_session_dynamodb::attr::{Item, n, s};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{MAX_ACTIONS, Participant};
use aex_test_harness::DynamoDbLocalContainer;
use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models::{LimitScalarValue, LimitValue};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};

/// The pinned physical authority table.
const AUTHORITY: &str = "dev-eu-west-1-regional-capacity-authority";
/// The pinned physical serving-projection table.
const PROJECTION: &str = "dev-eu-west-1-regional-authz-projection";

/// The audit participant, spelled as the adapter names it.
///
/// Written out rather than imported because the constant is private to the
/// store: a test that reached into the module could not tell a rename from a
/// regression, and this string is the one a caller actually receives.
const AUDIT: Participant = Participant::new("capacity.audit");

fn client(engine: &DynamoDbLocalContainer) -> Client {
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

/// Creates one composite-key table.
///
/// Both tables this adapter writes declare `pk`/`sk` and no global secondary
/// index, so the shape is stated once here rather than derived per table.
async fn create_table(client: &Client, table: &str) {
    let attribute = |name: &str| {
        AttributeDefinition::builder()
            .attribute_name(name)
            .attribute_type(ScalarAttributeType::S)
            .build()
            .expect("a complete attribute")
    };
    let key = |name: &str, kind: KeyType| {
        KeySchemaElement::builder()
            .attribute_name(name)
            .key_type(kind)
            .build()
            .expect("a complete key element")
    };
    client
        .create_table()
        .table_name(table)
        .set_attribute_definitions(Some(vec![attribute("pk"), attribute("sk")]))
        .set_key_schema(Some(vec![
            key("pk", KeyType::Hash),
            key("sk", KeyType::Range),
        ]))
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await
        .expect("the table is created");
}

async fn engine() -> (DynamoDbLocalContainer, Client, CapacityStore) {
    let engine = DynamoDbLocalContainer::start()
        .await
        .expect("DynamoDB Local starts");
    let client = client(&engine);
    create_table(&client, AUTHORITY).await;
    create_table(&client, PROJECTION).await;
    let store = CapacityStore::new(client.clone(), AUTHORITY, PROJECTION);
    (engine, client, store)
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

/// Every row of one table, grouped by `itemType`.
///
/// A scan is legitimate here and nowhere in production: the fixture table holds
/// exactly one workspace's rows, and the claim under test is about *how many*
/// rows one commit produced.
async fn rows_by_item_type(client: &Client, table: &str) -> BTreeMap<String, Vec<Item>> {
    let output = client
        .scan()
        .table_name(table)
        .consistent_read(true)
        .send()
        .await
        .expect("the scan succeeds");
    let mut grouped: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    for item in output.items.unwrap_or_default() {
        let item_type = item
            .get("itemType")
            .and_then(|value| value.as_s().ok())
            .expect("every row this adapter writes carries a discriminator")
            .clone();
        grouped.entry(item_type).or_default().push(item);
    }
    // Sorted by key, because a scan makes no ordering promise and two scans of
    // one unchanged table are compared directly below.
    for rows in grouped.values_mut() {
        rows.sort_by_key(|row| {
            (
                row.get("pk").and_then(|v| v.as_s().ok()).cloned(),
                row.get("sk").and_then(|v| v.as_s().ok()).cloned(),
            )
        });
    }
    grouped
}

/// The revision every row of one item type carries.
fn revisions(rows: &[Item]) -> Vec<u64> {
    rows.iter()
        .map(|row| {
            row.get("revision")
                .and_then(|value| value.as_n().ok())
                .expect("a projected row carries its revision")
                .parse::<u64>()
                .expect("a revision is an integer")
        })
        .collect()
}

async fn bootstrap(store: &CapacityStore) -> u64 {
    let defaults = canonical_defaults().expect("the embedded defaults parse");
    store
        .apply(
            &defaults,
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            at(1),
        )
        .await
        .expect("the bootstrap commits")
        .state
        .revision
}

fn override_api_body(expected_revision: u64, capacity_fence: u64, bytes: u128) -> CapacityCommand {
    CapacityCommand::SetOverride {
        workspace_id: workspace(),
        expected_revision,
        capacity_fence,
        approval_id: "support-approval-1".to_owned(),
        limit_id: LimitId::ApiJsonBody,
        value: LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(bytes),
        }),
    }
}

#[tokio::test]
async fn a_bootstrap_materialises_the_authority_the_audit_and_the_whole_projection_at_once() {
    let (_engine, client, store) = engine().await;
    let revision = bootstrap(&store).await;

    let authority = rows_by_item_type(&client, AUTHORITY).await;
    assert_eq!(authority["workspace_capacity"].len(), 1);
    assert_eq!(authority["capacity_audit"].len(), 1);

    let projection = rows_by_item_type(&client, PROJECTION).await;
    assert_eq!(
        projection["workspace_limit"].len(),
        LimitId::ALL.len(),
        "a partial projection is a workspace whose admission reads a limit the \
         authority never published"
    );
    assert_eq!(projection["workspace_limit_bundle"].len(), 1);
    assert_eq!(projection["workspace_limit_bundle_head"].len(), 1);
    assert_eq!(
        projection["workspace_edge_limits"].len(),
        1,
        "the hot admission subset is cut from the same bundle in the same \
         transaction, so an edge reading it alone cannot see an unpublished \
         revision"
    );

    let committed: usize = authority.values().map(Vec::len).sum::<usize>()
        + projection.values().map(Vec::len).sum::<usize>();
    assert_eq!(committed, 2 + LimitId::ALL.len() + 3);
    assert!(
        committed <= MAX_ACTIONS,
        "one bootstrap is one transaction, so the registry is capped at \
         {} limits and that ceiling is deliberate, not discovered",
        MAX_ACTIONS - 5
    );

    for item_type in [
        "workspace_limit",
        "workspace_limit_bundle",
        "workspace_limit_bundle_head",
        "workspace_edge_limits",
    ] {
        for observed in revisions(&projection[item_type]) {
            assert_eq!(
                observed, revision,
                "`{item_type}` published a revision the authority did not"
            );
        }
    }

    let loaded = store
        .load(workspace())
        .await
        .expect("the strong authority read succeeds")
        .expect("the authority row exists");
    assert_eq!(loaded.revision, revision);
    assert_eq!(loaded.effective.len(), LimitId::ALL.len());
    assert!(loaded.overrides.is_empty());
}

#[tokio::test]
async fn a_second_bootstrap_is_refused_by_the_authority_before_it_reaches_the_engine() {
    let (_engine, client, store) = engine().await;
    bootstrap(&store).await;
    let before = rows_by_item_type(&client, PROJECTION).await;

    let defaults = canonical_defaults().expect("the embedded defaults parse");
    let error = store
        .apply(
            &defaults,
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            at(2),
        )
        .await
        .expect_err("a workspace is bootstrapped once");
    assert!(
        matches!(
            error,
            CapacityStoreError::Capacity(CapacityError::AlreadyExists)
        ),
        "{error}"
    );
    assert_eq!(
        rows_by_item_type(&client, PROJECTION).await,
        before,
        "a refused command must not touch the serving projection"
    );
}

#[tokio::test]
async fn an_override_moves_the_authority_and_the_projection_to_one_new_revision() {
    let (_engine, client, store) = engine().await;
    let first = bootstrap(&store).await;
    let defaults = canonical_defaults().expect("the embedded defaults parse");

    let applied = store
        .apply(&defaults, &override_api_body(first, 1, 262_144), at(2))
        .await
        .expect("an approved override commits");
    assert!(applied.changed);
    assert_eq!(applied.state.revision, first + 1);
    assert_eq!(
        applied.state.overrides.get(&LimitId::ApiJsonBody),
        Some(&LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(262_144),
        }))
    );

    let projection = rows_by_item_type(&client, PROJECTION).await;
    for item_type in [
        "workspace_limit",
        "workspace_limit_bundle",
        "workspace_limit_bundle_head",
        "workspace_edge_limits",
    ] {
        assert!(
            revisions(&projection[item_type])
                .iter()
                .all(|observed| *observed == applied.state.revision),
            "`{item_type}` did not move with the authority; a bundle at one \
             revision and a head at another is exactly what the completeness \
             fence exists to prevent"
        );
    }
    assert_eq!(
        rows_by_item_type(&client, AUTHORITY).await["capacity_audit"].len(),
        2,
        "every committed decision keeps its own immutable audit row"
    );
}

#[tokio::test]
async fn a_command_planned_against_a_revision_the_authority_has_passed_is_refused() {
    let (_engine, _client, store) = engine().await;
    let first = bootstrap(&store).await;
    let defaults = canonical_defaults().expect("the embedded defaults parse");
    store
        .apply(&defaults, &override_api_body(first, 1, 262_144), at(2))
        .await
        .expect("the first override commits");

    let error = store
        .apply(&defaults, &override_api_body(first, 2, 131_072), at(3))
        .await
        .expect_err("an approval based on a revision that has moved");
    assert!(
        matches!(
            error,
            CapacityStoreError::Capacity(CapacityError::Revision { .. })
        ),
        "{error}"
    );
}

#[tokio::test]
async fn an_audit_row_that_already_exists_cancels_the_commit_and_moves_nothing() {
    let (_engine, client, store) = engine().await;
    let first = bootstrap(&store).await;
    let defaults = canonical_defaults().expect("the embedded defaults parse");
    let before = rows_by_item_type(&client, PROJECTION).await;

    // A previous attempt at this exact revision that committed its audit row.
    // The key template is written out rather than imported because it is
    // private to the store, and planting the collision is the only way to reach
    // the audit participant's condition from outside.
    client
        .put_item()
        .table_name(AUTHORITY)
        .item("pk", s(format!("WS#{}", workspace())))
        .item("sk", s(format!("AUDIT#{:020}", first + 1)))
        .item("itemType", s("capacity_audit"))
        .item("workspaceId", s(workspace().to_string()))
        .item("revision", n(first + 1))
        .send()
        .await
        .expect("the colliding audit row is written");

    let error = store
        .apply(&defaults, &override_api_body(first, 1, 262_144), at(2))
        .await
        .expect_err("an audited decision is never overwritten");
    let CapacityStoreError::Store(StoreError::PreconditionFailed { participant, .. }) = error else {
        panic!("an immutable audit collision must name its participant, not {error}");
    };
    assert_eq!(participant, AUDIT);

    assert_eq!(
        store
            .load(workspace())
            .await
            .expect("the read succeeds")
            .expect("the authority exists")
            .revision,
        first,
        "the authority must not advance past a decision whose audit row lost"
    );
    assert_eq!(
        rows_by_item_type(&client, PROJECTION).await,
        before,
        "a cancelled transaction must leave the serving projection exactly as it was"
    );
}
