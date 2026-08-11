//! Domain-to-wire projection tests for the launch surface.
//!
//! These prove projected values survive the generated wire schema and that
//! deliberately deferred values are refused at the boundary.

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_regional_http::cursor::{CursorError, SortTuple};
use aex_regional_http::projection::{
    ProjectionError, entity_tag, position_tuple, provider_credential, provider_credential_page,
    registered_file, registered_file_page, registered_file_row, session_message,
    session_message_page, tuple_position,
};
use aex_secret_custody_dynamodb::codec::{CredentialState, ProviderCredential as StoredCredential};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::secret::SourceGeneration;
use aex_session_dynamodb::paging::PagePosition;
use aex_wire::error::ErrorCode;
use aex_wire::ids::{
    ContentHash, PrefixedId as _, ProviderCredentialId, ResourceName, Uuid7, WorkspaceId,
};
use aex_wire::models;
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{
    RegistryPointer, RegistryRow as StoredRegistryRow, ValueDocument,
};

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

fn credential_id() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [6; 10]))
}

fn moment(spelling: &str) -> Timestamp {
    Timestamp::parse(spelling).expect("a pinned spelling")
}

fn stored_credential() -> StoredCredential {
    StoredCredential {
        credential: credential_id(),
        workspace: workspace(),
        name: ResourceName::parse("primary-openai").expect("a resource name"),
        provider: models::ProviderId::Openai,
        secret_name: ResourceName::parse("openai-key").expect("a resource name"),
        source_generation: SourceGeneration::FIRST,
        fingerprint: SecretPlaintext::new(b"sk-live-fixture".to_vec())
            .expect("a bounded plaintext")
            .credential_fingerprint(workspace(), credential_id()),
        revision: 3,
        state: CredentialState::Ready,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T13:00:00.000Z"),
        revoked_at: None,
    }
}

fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let encoded = serde_json::to_vec(value).expect("a projected model always encodes");
    serde_json::from_slice(&encoded).expect("a projected model always decodes as itself")
}

#[test]
fn sealed_messages_project_only_the_launch_part_variants() {
    let (session, _run, _agent, open) = aex_session_domain::testing::running_session();
    let mut sealed =
        aex_session_domain::seal(&open, aex_session_domain::testing::moment(10)).message;
    sealed.parts = vec![
        aex_session_domain::MessagePart::Text {
            text: "hello".to_owned(),
        },
        aex_session_domain::MessagePart::ToolCall {
            id: aex_session_domain::testing::id(21),
            arguments: ContentHash::of(b"arguments"),
        },
        aex_session_domain::MessagePart::ToolResult {
            id: aex_session_domain::testing::id(21),
            result: ContentHash::of(b"result"),
        },
    ];
    let page = session_message_page(&[sealed.clone()], None).expect("sealed page");
    assert_eq!(round_trip(&page), page);
    assert_eq!(page.items[0].session_id, session.id);
    assert_eq!(page.items[0].content.len(), 3);

    sealed.parts.push(aex_session_domain::MessagePart::File {
        path: aex_wire::ids::FilePath::parse("/report.json").expect("path"),
        media_type: Some("application/json".to_owned()),
    });
    assert!(matches!(
        session_message(&sealed),
        Err(ProjectionError::DeferredMessageAttachment { .. })
    ));
    assert!(matches!(
        session_message(&open),
        Err(ProjectionError::UnsealedMessage { .. })
    ));
}

#[test]
fn a_projected_credential_survives_the_wire_unchanged() {
    let stored = stored_credential();
    let projected = provider_credential(&stored);
    assert_eq!(round_trip(&projected), projected);
    assert_eq!(projected.id, stored.credential);
    assert_eq!(projected.name, stored.name);
    assert_eq!(projected.provider, models::ProviderId::Openai);
    assert_eq!(projected.revision, 3);
    assert_eq!(projected.state, models::ProviderCredentialState::Ready);
    assert_eq!(projected.updated_at, moment("2026-08-01T13:00:00.000Z"));
    assert_eq!(projected.fingerprint, stored.fingerprint);
}

#[test]
fn a_projected_credential_never_publishes_its_custody_reference() {
    let stored = stored_credential();
    let encoded =
        serde_json::to_string(&provider_credential(&stored)).expect("a projected model encodes");
    assert!(!encoded.contains(stored.secret_name.as_str()), "{encoded}");
    assert!(!encoded.contains("sourceGeneration"), "{encoded}");
}

#[test]
fn a_revoked_credential_carries_its_instant() {
    let mut revoked = stored_credential();
    revoked.state = CredentialState::Revoked;
    revoked.revoked_at = Some(moment("2026-08-01T15:00:00.000Z"));
    revoked.revision = 4;
    let projected = provider_credential(&revoked);
    assert_eq!(projected.state, models::ProviderCredentialState::Revoked);
    assert_eq!(projected.revoked_at, revoked.revoked_at);
    assert_eq!(round_trip(&projected), projected);
}

#[test]
fn a_credential_page_survives_the_wire_unchanged() {
    let page = provider_credential_page(&[stored_credential()], None);
    assert_eq!(round_trip(&page), page);
    assert_eq!(page.items.len(), 1);
}

#[test]
fn page_positions_round_trip_through_cursor_tuples() {
    let base = PagePosition {
        pk: format!("SEC#{}", workspace()),
        sk: "NAME#openai-key".to_owned(),
        index_pk: None,
        index_sk: None,
    };
    assert_eq!(
        tuple_position(&position_tuple(&base).expect("a bounded tuple")).expect("resumes"),
        base
    );

    let indexed = PagePosition {
        pk: "operation-partition".to_owned(),
        sk: "STATE".to_owned(),
        index_pk: Some("workspace-operations".to_owned()),
        index_sk: Some("2026-08-02T12:00:00.000Z#operation-key".to_owned()),
    };
    let tuple = position_tuple(&indexed).expect("a bounded tuple");
    assert_eq!(tuple.parts().len(), 4);
    assert_eq!(tuple_position(&tuple).expect("resumes"), indexed);
}

#[test]
fn a_tuple_of_the_wrong_arity_is_refused() {
    let tuple = SortTuple::new(vec!["only-one".to_owned()]).expect("a bounded tuple");
    let refusal = tuple_position(&tuple).expect_err("refused");
    assert_eq!(refusal, ProjectionError::Cursor(CursorError::Malformed));
    assert_eq!(refusal.code(), ErrorCode::InvalidCursor);
}

#[test]
fn entity_tags_are_deterministic_kind_separated_strong_validators() {
    let value = provider_credential(&stored_credential());
    let tag = entity_tag("ProviderCredential", &value).expect("a tag");
    assert_eq!(
        entity_tag("ProviderCredential", &value).expect("a tag"),
        tag
    );
    assert_ne!(
        entity_tag("ProviderCredential", &value).expect("a tag"),
        entity_tag("SomethingElse", &value).expect("a tag")
    );
    let rendered = tag.as_str();
    assert!(rendered.starts_with('"') && rendered.ends_with('"'));
    assert_eq!(rendered.len(), 66);
    assert!(!rendered.starts_with("W/"));

    let mut revoked = stored_credential();
    revoked.state = CredentialState::Revoked;
    revoked.revoked_at = Some(moment("2026-08-01T15:00:00.000Z"));
    assert_ne!(
        entity_tag("ProviderCredential", &provider_credential(&revoked)).expect("a tag"),
        tag
    );
}

fn value_document() -> ValueDocument {
    let payload = ContentHash::of(b"body").to_wire();
    let text = format!(
        r#"{{"mountPath":"/etc/motd","mediaType":"text/plain","mode":"0644",
            "content":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
    );
    ValueDocument::new(aex_wire::CanonicalJson::parse(&text).expect("valid JSON"))
}

fn registry_row(kind: RegistryKind, name: &str) -> StoredRegistryRow {
    StoredRegistryRow {
        workspace: workspace(),
        kind,
        name: ResourceName::parse(name).expect("a resource name"),
        revision: Revision(7),
        etag: ETag::parse("\"registry-7\"").expect("a strong validator"),
        sha256: ContentHash::of(b"body"),
        size_bytes: 4_096,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T13:00:00.000Z"),
    }
}

fn registry_pointer(kind: RegistryKind, name: &str) -> RegistryPointer {
    RegistryPointer {
        row: registry_row(kind, name),
        value_doc: value_document(),
    }
}

#[test]
fn a_projected_file_survives_the_wire_unchanged() {
    let projected =
        registered_file(&registry_pointer(RegistryKind::File, "motd")).expect("it projects");
    let encoded = serde_json::to_vec(&projected).expect("it encodes");
    let decoded: models::RegisteredFile =
        serde_json::from_slice(&encoded).expect("it decodes through deny_unknown_fields");
    assert_eq!(decoded, projected);
    assert_eq!(projected.revision, 7);
    assert_eq!(projected.size_bytes.get(), 4_096);
}

#[test]
fn a_file_collection_row_has_no_value() {
    let encoded = serde_json::to_value(
        registered_file_row(&registry_row(RegistryKind::File, "motd")).expect("it projects"),
    )
    .expect("it encodes");
    assert!(encoded.get("value").is_none());

    let complete = serde_json::to_value(
        registered_file(&registry_pointer(RegistryKind::File, "motd")).expect("it projects"),
    )
    .expect("it encodes");
    assert!(complete.get("value").is_some());
}

#[test]
fn the_file_projection_refuses_every_other_registry_kind() {
    for kind in [
        RegistryKind::File,
        RegistryKind::Skill,
        RegistryKind::Tool,
        RegistryKind::Instruction,
        RegistryKind::McpServer,
    ] {
        let pointer = registry_pointer(kind, "name");
        assert_eq!(
            registered_file(&pointer).is_ok(),
            kind == RegistryKind::File
        );
    }
}

#[test]
fn a_file_page_fails_whole_on_a_row_from_another_registry() {
    let mixed = [
        registry_row(RegistryKind::File, "motd"),
        registry_row(RegistryKind::Skill, "review"),
    ];
    assert!(registered_file_page(&mixed, None).is_err());
    assert_eq!(
        registered_file_page(&mixed[..1], None)
            .expect("a homogeneous page")
            .items
            .len(),
        1
    );
}
