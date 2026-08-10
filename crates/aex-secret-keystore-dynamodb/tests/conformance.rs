//! Request and generation-definition conformance for `regional-secret-keystore`.

mod support;

use aex_secret_keystore_dynamodb::branch_key;
use aex_secret_keystore_dynamodb::store::BranchKeyStoreReader;
use aex_session_dynamodb::paging::PageBudget;

use support::{DEFINITION, TABLE, branch_key_id, captured_body, capturing_reader};

fn definition() -> serde_json::Value {
    serde_json::from_str(DEFINITION).expect("the generation definition is JSON")
}

#[test]
fn the_key_schema_is_the_providers_and_not_the_workspace_convention() {
    let definition = definition();
    assert_eq!(
        definition["keySchema"]["partition"].as_str(),
        Some(branch_key::BRANCH_KEY_ID),
        "the hierarchical keyring defines these names; deviating would make the \
         store unreadable by the provider's own tooling"
    );
    assert_eq!(
        definition["keySchema"]["sort"].as_str(),
        Some(branch_key::TYPE)
    );
}

#[test]
fn nothing_about_this_table_can_expire_or_stream() {
    let definition = definition();
    assert_eq!(
        definition["timeToLive"]["enabled"].as_bool(),
        Some(false),
        "a branch key decrypts every ciphertext ever sealed under it; expiry \
         would silently destroy customer secrets"
    );
    assert_eq!(
        definition["stream"]["enabled"].as_bool(),
        Some(false),
        "wrapped branch-key material must never appear on a change feed"
    );
    assert_eq!(definition["deletionProtection"].as_bool(), Some(true));
}

/// The write boundary, restated after D-2 narrowed it.
///
/// Two roles may write and only two, and each only through
/// `TransactWriteItems`: every write here is the atomic version-plus-pointer
/// pair, so a bare `PutItem` or `UpdateItem` — which could move the active
/// pointer without the version row behind it — is granted to nobody at all.
/// `regional-secret-api` holds the transaction because it creates a workspace's
/// **first** branch key lazily on that workspace's first secret write, which is
/// one conditional transaction per workspace ever. Rotation stays exclusively
/// `regional-secret-key-admin`'s, enforced by the code path rather than by the
/// grant, because create and rotate are the same provider action.
#[test]
fn only_the_two_creating_roles_may_write_and_only_ever_atomically() {
    const MAY_WRITE: [&str; 2] = ["regional-secret-key-admin", "regional-secret-api"];

    let definition = definition();
    let grants = definition["iam"].as_array().expect("an array");
    for grant in grants {
        let role = grant["role"].as_str().expect("a role");
        let actions: Vec<&str> = grant["actions"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|action| action.as_str().expect("a string"))
            .collect();
        assert!(
            !actions
                .iter()
                .any(|action| action.contains("PutItem") || action.contains("UpdateItem")),
            "`{role}` could move the active pointer without its version row: {actions:?}"
        );
        assert_eq!(
            actions.contains(&"dynamodb:TransactWriteItems"),
            MAY_WRITE.contains(&role),
            "`{role}` holds the wrong side of the write boundary: {actions:?}"
        );
    }
}

#[tokio::test]
async fn describing_the_active_key_is_a_strongly_consistent_point_read() {
    let (reader, receiver) = capturing_reader();
    let _ignored = reader.describe_active(&branch_key_id(1)).await;

    let body = captured_body(receiver);
    assert_eq!(body["TableName"].as_str(), Some(TABLE));
    assert_eq!(body["ConsistentRead"].as_bool(), Some(true));
    assert_eq!(
        body["Key"][branch_key::TYPE]["S"].as_str(),
        Some(branch_key::ACTIVE)
    );
    assert_eq!(
        body["Key"][branch_key::BRANCH_KEY_ID]["S"].as_str(),
        Some(branch_key_id(1).as_str())
    );
}

#[tokio::test]
async fn listing_projects_identities_and_never_the_wrapped_material() {
    let (reader, receiver) = capturing_reader();
    let _ignored = reader
        .list_branch_key_ids(PageBudget::new(25).expect("a page"))
        .await;

    let body = captured_body(receiver);
    let projection = body["ProjectionExpression"].as_str().expect("a projection");
    assert_eq!(projection, "#id, #type");
    let names = body["ExpressionAttributeNames"]
        .as_object()
        .expect("attribute names");
    assert!(
        !names.values().any(|value| value.as_str() == Some("enc")),
        "an administrator's drift check must never pull key material back"
    );
}
