//! URL-ingest admission and stream-filter contract tests.

use std::collections::HashMap;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_wire::CanonicalJson;
use aex_wire::ids::{OrganizationId, PrefixedId as _, ResourceName, Uuid7, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{RegistryPointer, RegistryRow, RegistryState, ValueDocument};
use file_ingest_worker::{FetchFailure, PendingError, pending_url, stream_candidate};

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
fn stream_filter_is_closed_to_pending_file_pointers() {
    let mut item = HashMap::new();
    item.insert(
        "itemType".to_owned(),
        serde_dynamo::AttributeValue::S("registry_pointer".to_owned()),
    );
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [3; 10]));
    item.insert(
        "workspaceId".to_owned(),
        serde_dynamo::AttributeValue::S(workspace.to_string()),
    );
    item.insert(
        "kind".to_owned(),
        serde_dynamo::AttributeValue::S("file".to_owned()),
    );
    item.insert(
        "name".to_owned(),
        serde_dynamo::AttributeValue::S("a.bin".to_owned()),
    );
    item.insert(
        "state".to_owned(),
        serde_dynamo::AttributeValue::S("pending".to_owned()),
    );
    assert_eq!(
        stream_candidate(item.into())
            .expect("candidate")
            .name
            .as_str(),
        "a.bin"
    );
}
