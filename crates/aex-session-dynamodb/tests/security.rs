//! Security cases: what a row, an index and a diagnostic may never carry, and
//! what the projection module may never do.
//!
//! Each of these is a property a review would otherwise have to re-establish by
//! reading, which is exactly the kind of guarantee that decays.

mod support;

use std::path::{Path, PathBuf};

use aex_session_dynamodb::paging::CursorKey;
use aex_session_dynamodb::transactions::{AdmissionForeign, compile_admission};
use aex_session_dynamodb::wire_pending::SessionLifecycle;
use aex_session_dynamodb::{codec, keys};

use support::{admission, head, tables, workspace};

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
fn the_declared_index_projection_carries_no_body_prompt_config_or_receipt() {
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
        "resolvedConfig",
        "resolvedConfigDigest",
        "responseInline",
        "responseDigest",
        "intentHash",
    ] {
        assert!(
            !projected.contains(&forbidden),
            "the workspace index projects `{forbidden}`, so a list query would read it"
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
fn a_purged_head_is_physically_absent_from_the_workspace_index() {
    let mut purged = head();
    purged.lifecycle = SessionLifecycle::Purged;
    let encoded = codec::encode_head(&purged);
    assert!(!encoded.contains_key(keys::workspace_index::PK));
    assert!(!encoded.contains_key(keys::workspace_index::SK));
    // The head itself survives so one point read still serves `410 session_deleted`.
    assert!(encoded.contains_key("sessionId"));
    assert!(encoded.contains_key("deletionEpoch"));
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

#[test]
fn the_compiled_admission_writes_only_the_four_declared_tables() {
    let plan =
        compile_admission(&tables(), &admission(), AdmissionForeign::default()).expect("compiles");
    let tables = tables();
    for action in plan.actions() {
        let table = action
            .put()
            .map(|put| put.table_name().to_owned())
            .or_else(|| action.update().map(|u| u.table_name().to_owned()))
            .or_else(|| action.delete().map(|d| d.table_name().to_owned()))
            .or_else(|| action.condition_check().map(|c| c.table_name().to_owned()))
            .expect("every action names a table");
        assert!(
            [
                &tables.session_authority,
                &tables.regional_work,
                &tables.regional_content,
                &tables.regional_authz_projection,
            ]
            .contains(&&table),
            "the admission reached `{table}`"
        );
    }
}
