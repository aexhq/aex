//! Security cases: what a row, an index and a diagnostic may never carry, and
//! what the projection module may never do.
//!
//! Each of these is a property a review would otherwise have to re-establish by
//! reading, which is exactly the kind of guarantee that decays.

mod support;

use std::path::{Path, PathBuf};

use aex_session_dynamodb::paging::CursorKey;
use aex_session_dynamodb::{codec, keys};

use support::workspace;

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn table_definition() -> serde_json::Value {
    let text = std::fs::read_to_string(
        crate_root().join("../../migrations/regional/tables/session-authority.json"),
    )
    .expect("the generation definition is checked in");
    serde_json::from_str(&text).expect("the definition is JSON")
}

#[test]
fn the_workspace_index_is_a_keys_only_locator_that_can_never_return_a_body() {
    let definition = table_definition();
    let index = &definition["globalSecondaryIndexes"][0];
    let projected: Vec<&str> = index["projection"]["attributes"]
        .as_array()
        .expect("an exhaustive attribute list")
        .iter()
        .map(|value| value.as_str().expect("an attribute name"))
        .collect();
    assert_eq!(index["projection"]["type"].as_str(), Some("KEYS_ONLY"));
    assert!(
        projected.is_empty(),
        "a workspace index row is only an ordered locator; the authority row is hydrated before use"
    );
    assert_eq!(index["projectsRecordBody"].as_bool(), Some(false));
}

#[test]
fn no_index_on_this_table_projects_all() {
    let definition = table_definition();
    for index in definition["globalSecondaryIndexes"]
        .as_array()
        .expect("an index list")
    {
        assert_ne!(
            index["projection"]["type"].as_str(),
            Some("ALL"),
            "`ALL` lets a list query start returning a prompt the moment somebody adds an \
             attribute"
        );
        assert_eq!(index["sparse"].as_bool(), Some(true));
    }
}

#[test]
fn the_session_api_role_can_query_the_index_cancel_and_resolve_by_point_read() {
    let definition = table_definition();
    let grant = definition["iam"]
        .as_array()
        .expect("an IAM grant list")
        .iter()
        .find(|grant| grant["role"].as_str() == Some("regional-session-api"))
        .expect("the regional session API grant");
    let actions = grant["actions"]
        .as_array()
        .expect("an action list")
        .iter()
        .map(|action| action.as_str().expect("an action"))
        .collect::<Vec<_>>();
    for required in [
        "dynamodb:GetItem",
        "dynamodb:Query",
        "dynamodb:TransactWriteItems",
    ] {
        assert!(
            actions.contains(&required),
            "the route set requires {required}"
        );
    }
    assert!(
        !actions.contains(&"dynamodb:BatchGetItem"),
        "operation hydration deliberately uses the already-granted strong GetItem"
    );
    assert_eq!(
        grant["resources"],
        serde_json::json!(["table", "index/*"]),
        "the sparse operation GSI must be reachable without naming it twice"
    );
}

#[test]
fn only_ephemeral_replay_scaffolding_is_reclaimed_by_ttl() {
    let definition = table_definition();
    let applies_to: Vec<&str> = definition["timeToLive"]["appliesTo"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|value| value.as_str().expect("an item type"))
        .collect();
    assert_eq!(
        applies_to,
        vec![
            codec::IDEMPOTENCY_RECEIPT,
            "session_create_preparation",
            "session_create_prepared_file",
            "live_file_transfer",
            "live_file_election",
        ]
    );
}

#[test]
fn the_retired_session_stream_exposes_no_change_feed() {
    let definition = table_definition();
    assert_eq!(definition["stream"]["viewType"].as_str(), Some("NONE"));
    assert_eq!(
        definition["stream"]["consumers"]
            .as_array()
            .expect("a consumer list")
            .len(),
        0,
        "the removed observation stream leaves no consumer"
    );
}

#[test]
fn a_cursor_signing_key_never_prints_its_material() {
    let key = CursorKey::new(vec![1u8; 32]).expect("a long enough key");
    let rendered = format!("{key:?}");
    assert!(!rendered.contains('1'), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
}

#[test]
fn a_receipt_response_body_never_reaches_a_key() {
    let receipt_key =
        keys::receipt(workspace(), "session.create", &"a".repeat(64)).expect("a valid receipt key");
    assert!(receipt_key.pk.starts_with("IDEM#"));
    assert_eq!(receipt_key.sk, "RECEIPT");
    assert!(!receipt_key.pk.contains("response"));
}
