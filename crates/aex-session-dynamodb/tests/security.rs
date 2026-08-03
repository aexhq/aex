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
fn the_projection_module_contains_no_write_operation_at_all() {
    // `regional-authz-projection` is written only by `central-control-worker`.
    // Read-only by convention is not a property, so it is asserted here.
    let source = std::fs::read_to_string(crate_root().join("src/projection.rs"))
        .expect("the projection module is checked in");
    for forbidden in [
        "put_item",
        "update_item",
        "delete_item",
        "transact_write_items",
        "batch_write_item",
    ] {
        assert!(
            !source.contains(forbidden),
            "the projection reader calls `{forbidden}`; that table is read-only for every \
             regional role"
        );
    }
}

#[test]
fn the_session_index_projects_one_complete_session_document_but_no_message_or_receipt_body() {
    let definition = table_definition();
    let projected: Vec<&str> = definition["globalSecondaryIndexes"][0]["projection"]["attributes"]
        .as_array()
        .expect("an exhaustive attribute list")
        .iter()
        .map(|value| value.as_str().expect("an attribute name"))
        .collect();
    for forbidden in [
        "contentInline",
        "contentDigest",
        "bodyInline",
        "bodyDigest",
        "responseInline",
        "responseDigest",
        "intentHash",
    ] {
        assert!(
            !projected.contains(&forbidden),
            "the workspace index projects `{forbidden}`, so a list query would read it"
        );
    }
    for required in [
        "authoritySchemaVersion",
        "authorityDocument",
        "resolvedConfigDigest",
    ] {
        assert!(
            projected.contains(&required),
            "a complete session page requires `{required}` without N+1 hydration"
        );
    }
}

#[test]
fn no_index_on_this_table_projects_all() {
    let definition = table_definition();
    for index in definition["globalSecondaryIndexes"]
        .as_array()
        .expect("an index list")
    {
        assert_eq!(
            index["projection"]["type"].as_str(),
            Some("INCLUDE"),
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
fn only_the_idempotency_receipt_is_reclaimed_by_ttl() {
    let definition = table_definition();
    let applies_to: Vec<&str> = definition["timeToLive"]["appliesTo"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|value| value.as_str().expect("an item type"))
        .collect();
    assert_eq!(applies_to, vec![codec::IDEMPOTENCY_RECEIPT]);
}

#[test]
fn the_stream_view_type_is_keys_only_so_a_consumer_learns_no_content() {
    let definition = table_definition();
    assert_eq!(definition["stream"]["viewType"].as_str(), Some("KEYS_ONLY"));
    assert_eq!(
        definition["stream"]["consumers"]
            .as_array()
            .expect("a consumer list")
            .len(),
        1,
        "exactly one consumer reads this stream"
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
