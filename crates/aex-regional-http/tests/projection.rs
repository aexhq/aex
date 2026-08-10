//! The domain-to-wire projection, proved by round trips rather than by field
//! copies.
//!
//! A field-copy assertion restates the implementation. What matters is that the
//! projected value **survives the wire**: it encodes to the published schema,
//! decodes back through `deny_unknown_fields`, and comes out equal. That is one
//! assertion per model that catches a renamed member, a missing member, an
//! unexpected member and a serializer disagreement at once.
//!
//! The refusals are asserted separately, because "what the projection refuses to
//! say" is the half a round trip cannot see.

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_regional_http::cursor::{CursorError, SortTuple};
use aex_regional_http::projection::{
    ProjectionError, entity_tag, position_tuple, provider_credential, provider_credential_page,
    registered_file, registered_file_row, registered_instruction, registered_instruction_row,
    registered_mcp_server, registered_mcp_server_row, registered_skill, registered_skill_page,
    registered_tool, secret_metadata, secret_metadata_page, secret_plaintext, secret_revocation,
    session_message, session_message_page, session_run, session_run_page, tuple_position,
};
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential as StoredCredential, SecretMetadata as StoredSecret,
};
use aex_secret_domain::plaintext::{PlaintextError, SecretPlaintext};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SecretState, SourceGeneration};
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

// --- fixtures ---------------------------------------------------------------------

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

fn credential_id() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [6; 10]))
}

fn moment(spelling: &str) -> Timestamp {
    Timestamp::parse(spelling).expect("a pinned spelling")
}

fn stored_secret() -> StoredSecret {
    StoredSecret {
        workspace: workspace(),
        name: ResourceName::parse("openai-key").expect("a resource name"),
        generation: SourceGeneration::FIRST,
        revision: SecretRevision::FIRST,
        state: SecretState::Ready,
        revocation_epoch: RevocationEpoch::INITIAL,
        revoked_through_revision: SecretRevision(0),
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
        revoked_at: None,
    }
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

/// Encodes a projected model and decodes it back through the published schema.
///
/// Every generated model is `deny_unknown_fields`, so this fails on a member the
/// schema does not declare as loudly as it fails on a missing one.
fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let encoded = serde_json::to_vec(value).expect("a projected model always encodes");
    serde_json::from_slice(&encoded).expect("a projected model always decodes as itself")
}

// --- sessions ---------------------------------------------------------------------

#[test]
fn a_complete_sealed_message_survives_every_public_part_variant() {
    let (session, _run, _agent, open) = aex_session_domain::testing::running_session();
    let mut sealed =
        aex_session_domain::seal(&open, aex_session_domain::testing::moment(10)).message;
    sealed.parts = vec![
        aex_session_domain::MessagePart::Text {
            text: "hello".to_owned(),
        },
        aex_session_domain::MessagePart::File {
            path: aex_wire::ids::FilePath::parse("/report.json").expect("path"),
            media_type: Some("application/json".to_owned()),
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
    assert_eq!(page.items[0].content.len(), 4);

    assert!(matches!(
        session_message(&open),
        Err(ProjectionError::UnsealedMessage { .. })
    ));
}

#[test]
fn a_failed_canonical_run_survives_the_public_wire() {
    let (_session, mut run, _agent, _message) = aex_session_domain::testing::running_session();
    run.status = aex_session_domain::RunStatus::Failed;
    run.terminal_at = Some(aex_session_domain::testing::moment(10));
    run.outcome = Some(aex_session_domain::RunOutcome::Failed {
        error: aex_session_domain::DomainError {
            code: ErrorCode::UpstreamError,
            message: "provider unavailable".to_owned(),
            detail: Some(aex_wire::CanonicalJson::parse(r#"{"phase":"dispatch"}"#).expect("JSON")),
            retryable: true,
        },
    });
    run.telemetry_complete = Some(false);
    run.telemetry_gaps = Some(vec![aex_session_domain::testing::id(12)]);

    let projected = session_run(&run);
    assert_eq!(round_trip(&projected), projected);
    assert_eq!(projected.status, models::RunStatus::Failed);
    let error = projected.error.expect("a typed failure");
    assert_eq!(error.code.known(), Some(ErrorCode::UpstreamError));
    assert!(error.retryable);
    assert_eq!(projected.telemetry_complete, Some(false));
}

#[test]
fn only_a_successful_run_publishes_output_message_ids() {
    let (_session, mut run, _agent, _message) = aex_session_domain::testing::running_session();
    let output = aex_session_domain::testing::id(13);
    run.status = aex_session_domain::RunStatus::Succeeded;
    run.terminal_at = Some(aex_session_domain::testing::moment(10));
    run.outcome = Some(aex_session_domain::RunOutcome::Succeeded {
        output_messages: vec![output],
    });

    let page = session_run_page(&[run], None);
    assert_eq!(round_trip(&page), page);
    assert_eq!(page.items[0].output_message_ids, Some(vec![output]));
    assert!(page.items[0].error.is_none());
}

// --- secrets ------------------------------------------------------------------------

#[test]
fn a_projected_secret_survives_the_wire_unchanged() {
    let projected = secret_metadata(&stored_secret()).expect("a live record projects");
    assert_eq!(round_trip(&projected), projected);

    // The value is nowhere in the representation, and neither is anything the
    // authority holds but does not publish.
    let encoded = serde_json::to_value(&projected).expect("encodes");
    let members: Vec<&str> = encoded
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        ["createdAt", "name", "revision", "state", "updatedAt"],
        "the projection publishes exactly the declared members"
    );
    assert_eq!(projected.name.as_str(), "openai-key");
    assert_eq!(projected.revision, 1);
    assert_eq!(projected.state, models::SecretState::Ready);
}

/// An emergency revoke is one conditional update that raises the fence and
/// leaves the stored `state` and `revision` alone, so a projection that copied
/// the state column would tell a customer `ready` about a record nothing may
/// use. The public state is read from the comparison the admission path uses.
#[test]
fn a_fenced_secret_projects_as_revoked_even_though_its_state_column_is_ready() {
    let mut fenced = stored_secret();
    fenced.revoked_through_revision = fenced.revision;
    fenced.revoked_at = Some(moment("2026-08-01T14:00:00.000Z"));
    assert_eq!(fenced.state, SecretState::Ready, "the column is untouched");

    let projected = secret_metadata(&fenced).expect("a fenced record still has a representation");
    assert_eq!(projected.state, models::SecretState::Revoked);
    assert_eq!(projected.revoked_at, fenced.revoked_at);
    assert_eq!(round_trip(&projected), projected);
}

/// A later `set` mints a generation above the fence, so the record is admissible
/// again and must say so.
#[test]
fn a_secret_replaced_after_a_revoke_projects_as_ready_again() {
    let mut replaced = stored_secret();
    replaced.revoked_through_revision = SecretRevision(1);
    replaced.revision = SecretRevision(2);
    assert_eq!(
        secret_metadata(&replaced).expect("projects").state,
        models::SecretState::Ready
    );
}

#[test]
fn a_deleted_secret_has_no_public_representation() {
    let mut deleted = stored_secret();
    deleted.state = SecretState::Deleted;
    let refusal = secret_metadata(&deleted).expect_err("a tombstone is refused");
    assert!(matches!(refusal, ProjectionError::SecretDeleted { .. }));
    assert_eq!(
        refusal.code(),
        ErrorCode::NotFound,
        "a tombstone is absent, never a `deleted` state on the wire"
    );
}

#[test]
fn a_listing_skips_a_tombstone_rather_than_failing_the_whole_page() {
    let mut deleted = stored_secret();
    deleted.state = SecretState::Deleted;
    deleted.name = ResourceName::parse("gone").expect("a resource name");
    let page = secret_metadata_page(&[stored_secret(), deleted], None).expect("a page projects");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].name.as_str(), "openai-key");
    assert_eq!(page.next_cursor, None);
    assert_eq!(round_trip(&page), page);
}

#[test]
fn a_revocation_receipt_needs_a_committed_revocation_instant() {
    let refusal = secret_revocation(&stored_secret()).expect_err("no instant, no receipt");
    assert!(matches!(refusal, ProjectionError::NotRevoked { .. }));

    let mut revoked = stored_secret();
    revoked.revoked_at = Some(moment("2026-08-01T14:00:00.000Z"));
    revoked.revoked_through_revision = revoked.revision;
    let receipt = secret_revocation(&revoked).expect("a committed revocation projects");
    assert_eq!(receipt.revoked_at, moment("2026-08-01T14:00:00.000Z"));
    assert_eq!(receipt.revision, 1);
    assert_eq!(round_trip(&receipt), receipt);
}

#[test]
fn a_put_body_becomes_a_zeroizing_plaintext_and_nothing_else() {
    let plaintext = secret_plaintext(models::SecretPutRequest {
        value: "hunter2".to_owned(),
    })
    .expect("a bounded value");
    assert_eq!(plaintext.expose_for_encryption(), b"hunter2");
    assert!(!format!("{plaintext:?}").contains("hunter2"));

    let refusal = secret_plaintext(models::SecretPutRequest {
        value: String::new(),
    })
    .expect_err("an empty value is refused");
    assert_eq!(refusal, ProjectionError::Plaintext(PlaintextError::Empty));
    assert_eq!(refusal.code(), ErrorCode::InvalidRequest);
}

// --- provider credentials -------------------------------------------------------------

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
    assert_eq!(
        projected.fingerprint, stored.fingerprint,
        "the fingerprint is the persisted one; nothing here can recompute it"
    );
}

/// The binding names a workspace secret, and that name is authority-internal:
/// publishing it would let a reader of the credential directory address the
/// custody row that holds the key.
#[test]
fn a_projected_credential_never_publishes_the_secret_it_references() {
    let stored = stored_credential();
    let encoded =
        serde_json::to_string(&provider_credential(&stored)).expect("a projected model encodes");
    assert!(
        !encoded.contains(stored.secret_name.as_str()),
        "the referenced workspace secret must not reach the wire: {encoded}"
    );
    assert!(
        !encoded.contains("sourceGeneration"),
        "the bound generation is a custody fact, not a public one: {encoded}"
    );
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

// --- continuations ------------------------------------------------------------------

#[test]
fn a_page_position_round_trips_through_the_cursor_tuple() {
    let position = PagePosition {
        pk: format!("SEC#{}", workspace()),
        sk: "NAME#openai-key".to_owned(),
        index_pk: None,
        index_sk: None,
    };
    let tuple = position_tuple(&position).expect("a bounded tuple");
    assert_eq!(tuple_position(&tuple).expect("resumes"), position);
}

#[test]
fn an_index_page_position_round_trips_with_every_last_evaluated_key_part() {
    let position = PagePosition {
        pk: "operation-partition".to_owned(),
        sk: "STATE".to_owned(),
        index_pk: Some("workspace-operations".to_owned()),
        index_sk: Some("2026-08-02T12:00:00.000Z#operation-key".to_owned()),
    };
    let tuple = position_tuple(&position).expect("a bounded index tuple");
    assert_eq!(tuple.parts().len(), 4);
    assert_eq!(tuple_position(&tuple).expect("resumes"), position);
}

#[test]
fn a_tuple_of_the_wrong_arity_is_refused_rather_than_padded() {
    let tuple = SortTuple::new(vec!["only-one".to_owned()]).expect("a bounded tuple");
    let refusal = tuple_position(&tuple).expect_err("refused");
    assert_eq!(refusal, ProjectionError::Cursor(CursorError::Malformed));
    assert_eq!(refusal.code(), ErrorCode::InvalidCursor);
}

// --- entity tags --------------------------------------------------------------------

/// The tag is derived from the representation, so it moves with every published
/// change — including a revoke, which advances no revision at all.
#[test]
fn an_entity_tag_moves_with_every_published_field() {
    let base = secret_metadata(&stored_secret()).expect("projects");
    let tag = entity_tag("SecretMetadata", &base).expect("a tag");
    assert_eq!(
        entity_tag("SecretMetadata", &base).expect("a tag"),
        tag,
        "the tag is deterministic"
    );

    let mut fenced = stored_secret();
    fenced.revoked_through_revision = fenced.revision;
    fenced.revoked_at = Some(moment("2026-08-01T14:00:00.000Z"));
    let revoked = secret_metadata(&fenced).expect("projects");
    assert_eq!(
        revoked.revision, base.revision,
        "an emergency revoke advances no revision"
    );
    assert_ne!(
        entity_tag("SecretMetadata", &revoked).expect("a tag"),
        tag,
        "a tag derived from a revision column would not have moved here"
    );
}

#[test]
fn two_kinds_carrying_equal_values_never_share_a_tag() {
    let value = secret_metadata(&stored_secret()).expect("projects");
    assert_ne!(
        entity_tag("SecretMetadata", &value).expect("a tag"),
        entity_tag("SomethingElse", &value).expect("a tag")
    );
}

#[test]
fn an_entity_tag_is_a_quoted_strong_validator() {
    let value = secret_metadata(&stored_secret()).expect("projects");
    let tag = entity_tag("SecretMetadata", &value).expect("a tag");
    let rendered = tag.as_str();
    assert!(
        rendered.starts_with('"') && rendered.ends_with('"'),
        "{rendered}"
    );
    assert_eq!(rendered.len(), 66, "a quoted SHA-256 in lowercase hex");
    assert!(
        rendered[1..65]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{rendered}"
    );
    assert!(
        !rendered.starts_with("W/"),
        "a weak validator would make If-Match unusable"
    );
}

// --- the registry ------------------------------------------------------------------

/// The value document each kind stores, so a point projection has something of
/// the right shape to decode.
fn value_document(kind: RegistryKind) -> ValueDocument {
    let payload = ContentHash::of(b"body").to_wire();
    let text = match kind {
        RegistryKind::File => format!(
            r#"{{"mountPath":"/etc/motd","mediaType":"text/plain","mode":"0644",
                "content":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Skill => format!(
            r#"{{"description":"review","bundleFormat":"tar.gz",
                "bundle":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Tool => format!(
            r#"{{"description":"search","inputSchema":{{"type":"object"}},"entry":"main.js",
                "bundleFormat":"tar.gz",
                "bundle":{{"sha256":"{payload}","sizeBytes":"4"}}}}"#
        ),
        RegistryKind::Instruction => r#"{"text":"be concise"}"#.to_owned(),
        RegistryKind::McpServer => {
            r#"{"url":"https://example.test/mcp","transport":"streamable_http","headers":[]}"#
                .to_owned()
        }
    };
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
        value_doc: value_document(kind),
    }
}

#[test]
fn a_projected_registry_row_survives_the_wire_unchanged() {
    let projected =
        registered_skill(&registry_pointer(RegistryKind::Skill, "review")).expect("it projects");
    let encoded = serde_json::to_vec(&projected).expect("it encodes");
    let decoded: models::RegisteredSkill =
        serde_json::from_slice(&encoded).expect("it decodes through deny_unknown_fields");
    assert_eq!(decoded, projected);
    assert_eq!(projected.revision, 7);
    assert_eq!(projected.size_bytes.get(), 4_096);
    assert_eq!(projected.state, models::RegisteredState::Current);
}

#[test]
fn a_collection_row_has_no_field_a_value_could_go_in() {
    // After the D-6 split this is structural rather than a convention: the row
    // types carry no `value` at all, so a listing cannot publish one even by
    // mistake, and the point types have no `None` to publish.
    for kind in [
        RegistryKind::File,
        RegistryKind::Instruction,
        RegistryKind::McpServer,
    ] {
        let row = registry_row(kind, "name");
        let encoded = match kind {
            RegistryKind::File => {
                serde_json::to_value(registered_file_row(&row).expect("it projects"))
            }
            RegistryKind::Instruction => {
                serde_json::to_value(registered_instruction_row(&row).expect("it projects"))
            }
            _ => serde_json::to_value(registered_mcp_server_row(&row).expect("it projects")),
        }
        .expect("it encodes");
        assert!(
            encoded.get("value").is_none(),
            "a collection row has no value field"
        );
    }

    // And the point form always has one.
    let complete = serde_json::to_value(
        registered_file(&registry_pointer(RegistryKind::File, "notes.md")).expect("it projects"),
    )
    .expect("it encodes");
    assert!(
        complete.get("value").is_some(),
        "a point response publishes the complete value"
    );
}

#[test]
fn a_pointer_from_another_registry_is_refused_rather_than_republished() {
    // All five registries share one table and one key template, so the kind is
    // the only thing separating two identically named rows. Publishing a skill
    // as a tool is the failure this refusal exists to prevent.
    let skill = registry_pointer(RegistryKind::Skill, "review");
    let error = registered_tool(&skill).expect_err("a skill is not a tool");
    assert_eq!(
        error,
        ProjectionError::WrongRegistryKind {
            expected: "tool",
            found: "skill"
        }
    );
    assert_eq!(error.code(), ErrorCode::InternalError);
}

#[test]
fn every_registry_projection_refuses_every_other_kind() {
    let kinds = [
        RegistryKind::File,
        RegistryKind::Skill,
        RegistryKind::Tool,
        RegistryKind::Instruction,
        RegistryKind::McpServer,
    ];
    for kind in kinds {
        let pointer = registry_pointer(kind, "name");
        assert_eq!(
            registered_file(&pointer).is_ok(),
            kind == RegistryKind::File,
            "file over {kind:?}"
        );
        assert_eq!(
            registered_skill(&pointer).is_ok(),
            kind == RegistryKind::Skill,
            "skill over {kind:?}"
        );
        assert_eq!(
            registered_tool(&pointer).is_ok(),
            kind == RegistryKind::Tool,
            "tool over {kind:?}"
        );
        assert_eq!(
            registered_instruction(&pointer).is_ok(),
            kind == RegistryKind::Instruction,
            "instruction over {kind:?}"
        );
        assert_eq!(
            registered_mcp_server(&pointer).is_ok(),
            kind == RegistryKind::McpServer,
            "mcp server over {kind:?}"
        );
    }
}

#[test]
fn a_registry_page_fails_whole_rather_than_dropping_a_row_it_cannot_project() {
    // Unlike a tombstone, which is genuinely absent, a row from the wrong
    // registry means the query and the key template disagreed. Skipping it would
    // publish a short page as a complete one.
    let mixed = [
        registry_row(RegistryKind::Skill, "review"),
        registry_row(RegistryKind::Tool, "search"),
    ];
    assert!(registered_skill_page(&mixed, None).is_err());
    assert_eq!(
        registered_skill_page(&mixed[..1], None)
            .expect("a homogeneous page")
            .items
            .len(),
        1
    );
}
