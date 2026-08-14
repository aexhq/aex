//! Regression coverage for the private durable create-preparation authority.

use aex_brain_domain::budget::DimensionVector;
use aex_brain_domain::ids::{CatalogPin, ModelSlug};
use aex_brain_domain::journal::JournalRecord;
use aex_brain_domain::wire_pending::{AgentLimits, ResolvedAgentConfig, SessionCredentialPin};
use aex_session_dynamodb::create_preparation::{
    CreatePreparation, CreatePreparationError, PreparedFile, RootStarted, STARTUP_FILE_MAX_BYTES,
    STARTUP_FILE_MAX_COUNT, decode_elected_preparation, decode_root_started, elect_plan,
    root_started_plan, stage_plans,
};
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{
    AgentId, ContentHash, FilePath, GenerationId, OrganizationId, PrefixedId as _,
    ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::models::RegisteredFileMode;
use aex_wire::provider::ProviderId;
use aex_wire::types::{ETag, Timestamp};

fn uuid(tag: u8) -> Uuid7 {
    Uuid7::compose(1_767_225_600_000 + u64::from(tag), [tag; 10])
}

fn file(index: usize, size_bytes: u64) -> PreparedFile {
    PreparedFile {
        name: ResourceName::parse(&format!("file-{index}")).expect("name"),
        revision: u64::try_from(index).expect("revision") + 1,
        etag: ETag::parse(&format!("etag-{index}")).expect("etag"),
        content: ContentHash::from_bytes([u8::try_from(index % 251).expect("tag"); 32]),
        size_bytes,
        mount_path: FilePath::parse(&format!("/src/file-{index}")).expect("path"),
        media_type: "application/octet-stream".to_owned(),
        mode: RegisteredFileMode::V0644,
    }
}

fn preparation(files: Vec<PreparedFile>) -> CreatePreparation {
    CreatePreparation {
        workspace: WorkspaceId::from_uuid7(uuid(1)),
        organization: OrganizationId::from_uuid7(uuid(2)),
        intent: IntentDigest::from_bytes([3; 32]),
        receipt_key_sha256: "04".repeat(32),
        coordinator: uuid(4),
        session: SessionId::from_uuid7(uuid(5)),
        root_agent: AgentId::from_uuid7(uuid(6)),
        generation: Some(GenerationId::from_uuid7(uuid(7))),
        files,
        root_record: root_record(GenerationId::from_uuid7(uuid(7))),
        runtime_definition: Some(br#"{"generation":"fixture"}"#.to_vec()),
        resolved_config: br#"{"model":"deepseek-chat"}"#.to_vec(),
        metadata: Some(br#"{"label":"fixture"}"#.to_vec()),
        materialized_agents: 8,
        account_revision: 11,
        prepared_at: Timestamp::from_unix_millis(1_767_225_600_008).expect("timestamp"),
    }
}

fn root_record(generation: GenerationId) -> JournalRecord {
    JournalRecord::AgentStarted {
        config: Box::new(ResolvedAgentConfig {
            catalog_pin: CatalogPin::from_wire(&format!("mc1_{}", "01".repeat(32)))
                .expect("catalog"),
            provider: ProviderId::Deepseek,
            credential: SessionCredentialPin::new(
                ProviderCredentialId::from_uuid7(uuid(8)),
                1,
                1,
                0,
            )
            .expect("credential"),
            model: ModelSlug::truncating("deepseek-chat"),
            system: None,
            tool_manifest_digests: Vec::new(),
            mcp_servers: Vec::new(),
            hands_generation: Some(generation),
            limits_revision: 1,
            limits: AgentLimits {
                turn_deadline_ms: 600_000,
                max_run_duration_ms: 3_600_000,
                max_depth: 4,
                max_fanout: 32,
            },
        }),
        parent: None,
        join: None,
        depth: 0,
        budget: DimensionVector::uniform(1_000_000),
    }
}

fn started() -> RootStarted {
    RootStarted {
        occurred_at: Timestamp::from_unix_millis(1_767_225_600_009).expect("timestamp"),
    }
}

#[test]
fn exact_file_count_and_aggregate_boundaries_are_admitted_before_launch() {
    let max_count = preparation(
        (0..STARTUP_FILE_MAX_COUNT)
            .map(|index| file(index, 1))
            .collect(),
    );
    assert_eq!(
        max_count.validate().expect("256 files pass").file_count,
        STARTUP_FILE_MAX_COUNT
    );

    let too_many = preparation(
        (0..=STARTUP_FILE_MAX_COUNT)
            .map(|index| file(index, 1))
            .collect(),
    );
    assert!(matches!(
        too_many.validate(),
        Err(CreatePreparationError::FileCount { .. })
    ));

    let exact = preparation(vec![file(0, STARTUP_FILE_MAX_BYTES)]);
    assert_eq!(
        exact.validate().expect("512 MiB passes").total_bytes,
        STARTUP_FILE_MAX_BYTES
    );
    let over = preparation(vec![file(0, STARTUP_FILE_MAX_BYTES + 1)]);
    assert!(matches!(
        over.validate(),
        Err(CreatePreparationError::FileBytes { .. })
    ));
}

#[test]
fn every_selected_revision_fact_changes_the_elected_manifest_identity() {
    let original = preparation(vec![file(0, 8)]);
    let digest = original.validate().expect("valid").selection_digest;
    let mut changed = original.clone();
    changed.files[0].revision += 1;
    assert_ne!(changed.validate().expect("valid").selection_digest, digest);
    changed = original.clone();
    changed.files[0].mount_path = FilePath::parse("/other/file-0").expect("path");
    assert_ne!(changed.validate().expect("valid").selection_digest, digest);
    changed = original;
    changed.files[0].content = ContentHash::of(b"replacement");
    assert_ne!(changed.validate().expect("valid").selection_digest, digest);
}

#[test]
fn every_non_file_winner_fact_changes_the_elected_authority_identity() {
    let original = preparation(vec![file(0, 8)]);
    let digest = original.validate().expect("valid").authority_digest;
    let mut changed = original.clone();
    changed.runtime_definition = original.runtime_definition.clone().map(|mut raw| {
        raw.push(b' ');
        raw
    });
    assert_ne!(changed.validate().expect("valid").authority_digest, digest);
    changed = original.clone();
    changed.resolved_config.push(b' ');
    assert_ne!(changed.validate().expect("valid").authority_digest, digest);
    changed = original.clone();
    changed.materialized_agents += 1;
    assert_ne!(changed.validate().expect("valid").authority_digest, digest);
    changed = original.clone();
    changed.account_revision += 1;
    assert_ne!(changed.validate().expect("valid").authority_digest, digest);
    changed = original;
    changed.prepared_at = Timestamp::from_unix_millis(1_767_225_600_009).expect("timestamp");
    assert_ne!(changed.validate().expect("valid").authority_digest, digest);
}

#[test]
fn replay_key_not_request_intent_addresses_the_private_election() {
    let first = preparation(vec![file(0, 8)]);
    let mut second = first.clone();
    second.receipt_key_sha256 = "05".repeat(32);

    let partition = |prepared: &CreatePreparation| {
        elect_plan("dev-session-authority", prepared)
            .expect("election")
            .actions()[0]
            .put()
            .expect("header")
            .item()
            .get("pk")
            .and_then(|value| value.as_s().ok())
            .cloned()
            .expect("partition")
    };
    assert_eq!(first.intent, second.intent, "the request body is unchanged");
    assert_ne!(
        partition(&first),
        partition(&second),
        "different Idempotency-Keys must elect different sessions even for one body"
    );

    let mut conflicting = first.clone();
    conflicting.intent = IntentDigest::from_bytes([9; 32]);
    assert_eq!(
        partition(&first),
        partition(&conflicting),
        "one reused key must reload the original winner so intent drift conflicts"
    );
}

#[test]
fn replay_key_digest_is_canonical_lowercase_hex() {
    let mut malformed = preparation(Vec::new());
    malformed.receipt_key_sha256 = "AB".repeat(32);
    assert_eq!(malformed.validate(), Err(CreatePreparationError::ReplayKey));
}

#[test]
fn metadata_is_staged_per_file_then_header_and_generation_are_elected_atomically() {
    let prepared = preparation(vec![file(0, 8), file(1, 9)]);
    let stages = stage_plans("dev-session-authority", &prepared).expect("stage plans");
    assert_eq!(stages.len(), 2);
    assert!(stages.iter().all(|plan| plan.len() == 1));
    assert!(stages.iter().all(|plan| {
        plan.actions()[0]
            .put()
            .and_then(|put| put.item().get("expiresAtEpochSeconds"))
            .is_some()
    }));

    let election = elect_plan("dev-session-authority", &prepared).expect("election plan");
    assert_eq!(
        election.len(),
        3,
        "header + physical root control + deletion locator"
    );
    assert_eq!(
        election
            .participants()
            .iter()
            .map(|participant| participant.as_str())
            .collect::<Vec<_>>(),
        [
            "session.create_preparation",
            "agent.root_control",
            "session.create_preparation_edge"
        ]
    );
    let control = election.actions()[1].put().expect("root control").item();
    assert!(
        election.actions()[0]
            .put()
            .expect("header")
            .item()
            .contains_key("expiresAtEpochSeconds")
    );
    assert_eq!(
        control
            .get("limitsRevision")
            .and_then(|value| value.as_n().ok())
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        control
            .get("maxRunDurationMs")
            .and_then(|value| value.as_n().ok())
            .map(String::as_str),
        Some("3600000")
    );
    let edge = election.actions()[2]
        .put()
        .expect("deletion locator")
        .item();
    assert_eq!(
        edge.get("sessionId").and_then(|value| value.as_s().ok()),
        Some(&prepared.session.to_string())
    );
    assert_eq!(
        edge.get("preparationPk")
            .and_then(|value| value.as_s().ok()),
        Some(&format!(
            "CREATE#{}#{}",
            prepared.workspace, prepared.receipt_key_sha256
        ))
    );
}

#[test]
fn the_physical_winner_reloads_every_elected_fact_without_volatile_reads() {
    let prepared = preparation(vec![file(0, 8), file(1, 9)]);
    let files = stage_plans("dev-session-authority", &prepared)
        .expect("stage plans")
        .into_iter()
        .map(|plan| plan.actions()[0].put().expect("staged put").item().clone())
        .collect::<Vec<_>>();
    let election = elect_plan("dev-session-authority", &prepared).expect("election plan");
    let header = election.actions()[0].put().expect("header put").item();
    assert_eq!(
        decode_elected_preparation(
            header,
            &files,
            prepared.workspace,
            &prepared.receipt_key_sha256,
        )
        .expect("the winner reloads"),
        prepared
    );

    assert!(matches!(
        decode_elected_preparation(
            header,
            &files[..1],
            prepared.workspace,
            &prepared.receipt_key_sha256,
        ),
        Err(CreatePreparationError::Corrupt { .. })
    ));
}

#[test]
fn sequence_zero_agent_started_is_durable_readiness_not_an_absent_journal() {
    let prepared = preparation(vec![file(0, 8)]);
    let started = started();
    let plan =
        root_started_plan("dev-session-authority", &prepared, &started).expect("root-start plan");
    assert_eq!(plan.len(), 2, "control update + immutable journal entry");
    let update = plan.actions()[0].update().expect("control update");
    assert!(update.update_expression().contains("hasJournal"));
    assert!(update.update_expression().contains("journalTailHash"));
    assert_eq!(
        update
            .expression_attribute_values()
            .and_then(|values| values.get(":tail"))
            .and_then(|value| value.as_n().ok())
            .map(String::as_str),
        Some("0")
    );
    let journal = plan.actions()[1].put().expect("journal put").item();
    assert_eq!(
        journal
            .get("kind")
            .and_then(|value| value.as_s().ok())
            .map(String::as_str),
        Some("agent_started")
    );
}

#[test]
fn the_post_append_control_and_journal_pair_proves_root_readiness() {
    let prepared = preparation(vec![file(0, 8)]);
    let summary = prepared.validate().expect("valid");
    let election = elect_plan("dev-session-authority", &prepared).expect("election plan");
    let mut control = election.actions()[1]
        .put()
        .expect("root control")
        .item()
        .clone();
    control.insert("revision".to_owned(), aex_session_dynamodb::attr::n(1));
    control.insert("journalTail".to_owned(), aex_session_dynamodb::attr::n(0));
    control.insert(
        "journalTailHash".to_owned(),
        aex_session_dynamodb::attr::s(summary.root_entry_id.to_hex()),
    );
    control.insert(
        "hasJournal".to_owned(),
        aex_session_dynamodb::attr::boolean(true),
    );
    control.insert("status".to_owned(), aex_session_dynamodb::attr::s("idle"));
    let root =
        root_started_plan("dev-session-authority", &prepared, &started()).expect("root-start plan");
    let journal = root.actions()[1].put().expect("initial journal").item();
    let evidence =
        decode_root_started(&control, journal, &prepared).expect("the exact pair proves readiness");
    assert_eq!(evidence.revision, 1);
    assert_eq!(evidence.journal_tail, 0);
    assert_eq!(evidence.journal_tail_hash, summary.root_entry_id.0);

    control.insert(
        "generationId".to_owned(),
        aex_session_dynamodb::attr::s(GenerationId::from_uuid7(uuid(99)).to_string()),
    );
    assert!(matches!(
        decode_root_started(&control, journal, &prepared),
        Err(CreatePreparationError::Corrupt { .. })
    ));
}
