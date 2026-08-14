//! URL-ingest admission and stream-filter contract tests.

use std::collections::{BTreeSet, HashMap};

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_wire::CanonicalJson;
use aex_wire::ids::{OrganizationId, PrefixedId as _, ResourceName, Uuid7, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{RegistryPointer, RegistryRow, RegistryState, ValueDocument};
use file_ingest_worker::{FetchFailure, PendingError, pending_url, stream_candidate};

const FILE_AUTHORITY: &str =
    include_str!("../../../migrations/regional/tables/regional-file-authority.json");

fn strings(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| entry.as_str().expect("string"))
        .collect()
}

fn pointer(source: &str) -> RegistryPointer {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
    let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
    let document = ValueDocument::new(
        CanonicalJson::from_value(&serde_json::json!({
            "mediaType": "application/octet-stream",
            "mode": "0644",
            "organizationId": organization,
            "source": { "type": source, "url": "https://example.com/a.bin" },
        }))
        .expect("canonical pending document"),
    );
    RegistryPointer {
        row: RegistryRow {
            workspace,
            kind: RegistryKind::File,
            name: ResourceName::parse("a.bin").expect("name"),
            revision: Revision::FIRST,
            etag: ETag::parse("0123456789abcdef0123456789abcdef").expect("etag"),
            sha256: document.digest(),
            size_bytes: document.size_bytes(),
            state: RegistryState::Pending,
            failure_code: None,
            created_at: Timestamp::from_unix_millis(0).expect("time"),
            updated_at: Timestamp::from_unix_millis(0).expect("time"),
        },
        value_doc: document,
    }
}

#[test]
fn only_pending_url_documents_are_admitted() {
    assert_eq!(
        pending_url(&pointer("url")).expect("url").source.kind,
        "url"
    );
    assert_eq!(
        pending_url(&pointer("upload")),
        Err(PendingError::NotPendingUrl)
    );
}

#[test]
fn stable_failure_codes_never_expose_transport_detail() {
    assert_eq!(FetchFailure::SourceRejected.code(), "source_rejected");
    assert_eq!(FetchFailure::SourceTooLarge.code(), "source_too_large");
    assert!(FetchFailure::SourceUnavailable.code().len() <= 64);
}

#[test]
fn keys_only_stream_filter_is_closed_to_file_pointer_keys() {
    let mut item = HashMap::new();
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [3; 10]));
    item.insert(
        "pk".to_owned(),
        serde_dynamo::AttributeValue::S(format!("REG#{workspace}#file")),
    );
    item.insert(
        "sk".to_owned(),
        serde_dynamo::AttributeValue::S("NAME#a.bin".to_owned()),
    );
    assert_eq!(
        stream_candidate(item.into())
            .expect("candidate")
            .name
            .as_str(),
        "a.bin"
    );
}

#[test]
fn keys_only_stream_filter_rejects_non_file_partitions_and_non_pointer_rows() {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [4; 10]));
    for (pk, sk) in [
        (format!("REG#{workspace}#skill"), "NAME#a.bin".to_owned()),
        (format!("REG#{workspace}#file"), "COUNT".to_owned()),
        (format!("CONTENT#{workspace}"), "NAME#a.bin".to_owned()),
    ] {
        let item = HashMap::from([
            ("pk".to_owned(), serde_dynamo::AttributeValue::S(pk)),
            ("sk".to_owned(), serde_dynamo::AttributeValue::S(sk)),
        ]);
        assert!(stream_candidate(item.into()).is_none());
    }
}

#[test]
fn unified_file_authority_declares_the_active_adapter_contract() {
    let definition: serde_json::Value =
        serde_json::from_str(FILE_AUTHORITY).expect("file authority definition");
    let declared: BTreeSet<&str> = strings(&definition["itemTypes"]).into_iter().collect();
    let active: BTreeSet<&str> = aex_content_dynamodb::keys::ITEM_TYPES
        .iter()
        .chain(aex_registry_dynamodb::keys::ITEM_TYPES)
        .copied()
        .collect();
    assert_eq!(declared, active, "migration and active row codecs diverged");

    assert_eq!(definition["stream"]["viewType"], "KEYS_ONLY");
    let indexes = definition["globalSecondaryIndexes"]
        .as_array()
        .expect("indexes");
    let gc = indexes
        .iter()
        .find(|index| index["name"] == aex_content_dynamodb::keys::GC_INDEX)
        .expect("GC index");
    assert_eq!(
        strings(&gc["projection"]["attributes"]),
        aex_content_dynamodb::keys::GC_PROJECTION
    );
    let expiry = indexes
        .iter()
        .find(|index| index["name"] == aex_content_dynamodb::keys::EXPIRY_INDEX)
        .expect("expiry index");
    assert_eq!(
        strings(&expiry["projection"]["attributes"]),
        aex_content_dynamodb::keys::EXPIRY_PROJECTION
    );
}
