//! Private authority for synchronous session-create preparation.
//!
//! Selected file bodies remain in the content object store. This module keeps
//! only their exact registry revision, entity tag, content digest, byte size,
//! mount path and mode. Each file row is staged immutably before one small
//! transaction elects the selection header and physical root-agent control.
//! The split is required because 256 legal 4 KiB mount paths cannot fit in one
//! DynamoDB item or one 100-action transaction.

use std::collections::BTreeSet;

use aex_brain_domain::budget::{DIMENSIONS, Dimension, DimensionVector};
use aex_brain_domain::journal::JournalRecord;
use aex_wire::canonical::to_jcs_string;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{
    AgentId, ContentHash, FilePath, GenerationId, OrganizationId, ResourceName, SessionId, Uuid7,
    WorkspaceId,
};
use aex_wire::models::RegisteredFileMode;
use aex_wire::types::{ETag, Timestamp};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{Put, Update};
use futures::{StreamExt as _, TryStreamExt as _};
use serde::{Deserialize, Serialize};

use crate::attr::{Item, ItemBuilder, Row, b, boolean, n, s, stamp};
use crate::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation_with_resolution,
};
use crate::plan::{IMMUTABLE, Participant, TransactionPlan};

/// The declared synchronous startup-file aggregate ceiling: exactly 512 MiB.
pub const STARTUP_FILE_MAX_BYTES: u64 = 536_870_912;
/// The declared synchronous startup-file count ceiling.
pub const STARTUP_FILE_MAX_COUNT: usize = 256;

const PREPARATION: &str = "session_create_preparation";
const PREPARED_FILE: &str = "session_create_prepared_file";
const AGENT_CONTROL: &str = "agent_control";
const JOURNAL_ENTRY: &str = "agent_journal";
const PREPARATION_PARTICIPANT: Participant = Participant::new("session.create_preparation");
const PREPARED_FILE_PARTICIPANT: Participant = Participant::new("session.create_prepared_file");
const PREPARATION_RECLAIM_AFTER_MS: i64 = 86_400_000;

/// One exact registry revision selected for synchronous materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreparedFile {
    /// Registered public name.
    pub name: ResourceName,
    /// Exact monotonic registry revision.
    pub revision: u64,
    /// Exact strong entity tag read with the revision.
    pub etag: ETag,
    /// SHA-256 identity of the immutable content body.
    pub content: ContentHash,
    /// Exact body length.
    pub size_bytes: u64,
    /// Exact guest destination selected by the registry value.
    pub mount_path: FilePath,
    /// Declared media type needed to reconstruct the elected public value.
    pub media_type: String,
    /// Exact POSIX mode selected by the registry value.
    pub mode: RegisteredFileMode,
}

/// All immutable facts elected before provider launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePreparation {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning/charged organization.
    pub organization: OrganizationId,
    /// Canonical create intent.
    pub intent: IntentDigest,
    /// Lowercase SHA-256 of the caller's idempotency key.
    ///
    /// This, not `intent`, elects the private preparation. Different keys that
    /// carry the same request are distinct creates; one key carrying another
    /// request must find this row so it can conflict.
    pub receipt_key_sha256: String,
    /// Fresh coordinator attempt; another coordinator never resumes its work.
    pub coordinator: Uuid7,
    /// Final public session identity.
    pub session: SessionId,
    /// Root agent identity.
    pub root_agent: AgentId,
    /// Exact Hands generation that launch and upload must use.
    pub generation: GenerationId,
    /// Exact file selection in caller order.
    pub files: Vec<PreparedFile>,
    /// Exact root fact later committed as journal sequence zero.
    pub root_record: JournalRecord,
    /// Canonical immutable provider/runtime definition.
    pub runtime_definition: Vec<u8>,
    /// Canonical release-qualified public configuration.
    pub resolved_config: Vec<u8>,
    /// Canonical caller metadata, when supplied.
    pub metadata: Option<Vec<u8>>,
    /// Root-plus-subagent ceiling used to derive the elected root budget.
    pub materialized_agents: u64,
    /// Account projection revision rechecked at final public publication.
    pub account_revision: u64,
    /// Preparation instant.
    pub prepared_at: Timestamp,
}

/// Validated selection and root-record identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparationSummary {
    /// Selected file count.
    pub file_count: usize,
    /// Aggregate declared payload bytes.
    pub total_bytes: u64,
    /// SHA-256 over the canonical ordered file metadata document.
    pub selection_digest: ContentHash,
    /// BLAKE3 over the canonical `AgentStarted` body.
    pub root_entry_id: aex_brain_domain::ids::ContentHash,
    /// SHA-256 over every immutable elected preparation fact.
    pub authority_digest: ContentHash,
}

/// Provider-authoritative time at which readiness was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootStarted {
    /// When the exact sequence-zero fact became durable.
    pub occurred_at: Timestamp,
}

/// Strongly verified physical sequence-zero readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableRootStarted {
    /// Control revision after the initial append.
    pub revision: u64,
    /// Exact immutable journal position, always zero for root creation.
    pub journal_tail: u64,
    /// BLAKE3 identity of the canonical `AgentStarted` body.
    pub journal_tail_hash: [u8; 32],
    /// Timestamp stored on the immutable journal record.
    pub occurred_at: Timestamp,
}

/// Why create preparation cannot be persisted or published.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CreatePreparationError {
    /// The caller replay-key digest cannot enter the private authority key.
    #[error("the create receipt key digest is not 64 lowercase hex characters")]
    ReplayKey,
    /// More than 256 files were selected.
    #[error("selected {found} startup files; the ceiling is {maximum}")]
    FileCount {
        /// Selected count.
        found: usize,
        /// Declared maximum.
        maximum: usize,
    },
    /// Aggregate selected bytes exceeded 512 MiB.
    #[error("selected {found} startup bytes; the ceiling is {maximum}")]
    FileBytes {
        /// Selected aggregate.
        found: u64,
        /// Declared maximum.
        maximum: u64,
    },
    /// Two selected entries cannot share one registry or guest identity.
    #[error("the startup selection repeats {kind} `{value}`")]
    Duplicate {
        /// Name or mount path.
        kind: &'static str,
        /// Repeated value.
        value: String,
    },
    /// The persisted root fact was not the exact elected generation's root start.
    #[error("the root AgentStarted record is invalid: {reason}")]
    RootRecord {
        /// Exact violated invariant.
        reason: &'static str,
    },
    /// Canonical metadata or journal encoding failed.
    #[error("create preparation could not be canonicalized: {0}")]
    Canonical(String),
    /// The shared transaction compiler refused a row or condition.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Elected storage disagreed with the schema or its own immutable digests.
    #[error("stored create preparation is corrupt: {reason}")]
    Corrupt {
        /// Exact invariant or codec failure.
        reason: String,
    },
}

impl CreatePreparation {
    /// Validates the full pre-launch boundary and derives immutable identities.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] before any provider effect when the
    /// selected count/bytes, uniqueness or root generation is invalid.
    pub fn validate(&self) -> Result<PreparationSummary, CreatePreparationError> {
        if self.receipt_key_sha256.len() != 64
            || !self
                .receipt_key_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CreatePreparationError::ReplayKey);
        }
        if self.files.len() > STARTUP_FILE_MAX_COUNT {
            return Err(CreatePreparationError::FileCount {
                found: self.files.len(),
                maximum: STARTUP_FILE_MAX_COUNT,
            });
        }
        let mut names = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut total_bytes = 0_u64;
        for file in &self.files {
            if !names.insert(file.name.as_str()) {
                return Err(CreatePreparationError::Duplicate {
                    kind: "registered name",
                    value: file.name.to_string(),
                });
            }
            if !paths.insert(file.mount_path.as_str()) {
                return Err(CreatePreparationError::Duplicate {
                    kind: "mount path",
                    value: file.mount_path.as_str().to_owned(),
                });
            }
            total_bytes = total_bytes.checked_add(file.size_bytes).ok_or(
                CreatePreparationError::FileBytes {
                    found: u64::MAX,
                    maximum: STARTUP_FILE_MAX_BYTES,
                },
            )?;
            if total_bytes > STARTUP_FILE_MAX_BYTES {
                return Err(CreatePreparationError::FileBytes {
                    found: total_bytes,
                    maximum: STARTUP_FILE_MAX_BYTES,
                });
            }
        }
        let JournalRecord::AgentStarted {
            config,
            parent,
            join,
            depth,
            ..
        } = &self.root_record
        else {
            return Err(CreatePreparationError::RootRecord {
                reason: "the first root record is not AgentStarted",
            });
        };
        if parent.is_some() || join.is_some() || *depth != 0 {
            return Err(CreatePreparationError::RootRecord {
                reason: "a root start must have no parent/join and depth zero",
            });
        }
        if config.hands_generation != self.generation {
            return Err(CreatePreparationError::RootRecord {
                reason: "the pinned Hands generation differs from the elected generation",
            });
        }
        if !self
            .root_record
            .fits_inline()
            .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?
        {
            return Err(CreatePreparationError::RootRecord {
                reason: "the AgentStarted body exceeds the durable inline journal ceiling",
            });
        }
        let selection = to_jcs_string(&self.files)
            .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
        let selection_digest = ContentHash::of(selection.as_bytes());
        let root_entry_id = self
            .root_record
            .content_hash()
            .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
        let authority = to_jcs_string(&PreparationAuthority {
            workspace: self.workspace,
            organization: self.organization,
            intent: self.intent,
            receipt_key_sha256: self.receipt_key_sha256.clone(),
            coordinator: self.coordinator.to_string(),
            session: self.session,
            root_agent: self.root_agent,
            generation: self.generation,
            selection_digest,
            root_entry_id: root_entry_id.to_hex(),
            runtime_definition: ContentHash::of(&self.runtime_definition),
            resolved_config: ContentHash::of(&self.resolved_config),
            metadata: self.metadata.as_deref().map(ContentHash::of),
            materialized_agents: self.materialized_agents,
            account_revision: self.account_revision,
            prepared_at: self.prepared_at,
        })
        .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
        Ok(PreparationSummary {
            file_count: self.files.len(),
            total_bytes,
            selection_digest,
            root_entry_id,
            authority_digest: ContentHash::of(authority.as_bytes()),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparationAuthority {
    workspace: WorkspaceId,
    organization: OrganizationId,
    intent: IntentDigest,
    receipt_key_sha256: String,
    coordinator: String,
    session: SessionId,
    root_agent: AgentId,
    generation: GenerationId,
    selection_digest: ContentHash,
    root_entry_id: String,
    runtime_definition: ContentHash,
    resolved_config: ContentHash,
    metadata: Option<ContentHash>,
    materialized_agents: u64,
    account_revision: u64,
    prepared_at: Timestamp,
}

/// Builds one immutable stage transaction per selected file.
///
/// Staging precedes election. A lost coordinator can leave only unreachable
/// metadata rows; no provider can launch because the elected header does not
/// exist. Exact replay is admitted by the selection digest and index.
///
/// # Errors
///
/// Returns [`CreatePreparationError`] for invalid selection metadata or a row
/// the shared transaction compiler refuses.
pub fn stage_plans(
    table: &str,
    prepared: &CreatePreparation,
) -> Result<Vec<TransactionPlan>, CreatePreparationError> {
    let summary = prepared.validate()?;
    prepared
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| stage_plan(table, prepared, summary, index, file))
        .collect()
}

fn stage_plan(
    table: &str,
    prepared: &CreatePreparation,
    summary: PreparationSummary,
    index: usize,
    file: &PreparedFile,
) -> Result<TransactionPlan, CreatePreparationError> {
    let partition = preparation_partition(prepared.workspace, &prepared.receipt_key_sha256);
    let selection = summary.selection_digest.to_wire();
    let reclaim_at = preparation_reclaim_epoch_seconds(prepared.prepared_at)?;
    let sort = format!("FILE#{selection}#{index:06}");
    let index_u64 = u64::try_from(index).map_err(|_| CreatePreparationError::FileCount {
        found: index,
        maximum: STARTUP_FILE_MAX_COUNT,
    })?;
    let item = ItemBuilder::new(PREPARED_FILE)
        .set(crate::attr::PK, s(partition))
        .set(crate::attr::SK, s(sort))
        .set("workspaceId", s(prepared.workspace.to_string()))
        .set("intentDigest", s(prepared.intent.to_string()))
        .set("receiptKeySha256", s(prepared.receipt_key_sha256.clone()))
        .set("selectionDigest", s(selection.clone()))
        .set("fileIndex", n(index_u64))
        .set("registeredName", s(file.name.to_string()))
        .set("registryRevision", n(file.revision))
        .set("registryEtag", s(file.etag.to_string()))
        .set("contentDigest", s(file.content.to_wire()))
        .set("contentBytes", n(file.size_bytes))
        .set("mountPath", s(file.mount_path.as_str()))
        .set("mediaType", s(file.media_type.clone()))
        .set("mode", s(file.mode.as_str()))
        .set("expiresAtEpochSeconds", crate::attr::n_i64(reclaim_at))
        .build();
    let same_file = "attribute_not_exists(pk) OR (workspaceId = :workspace AND receiptKeySha256 = :receipt AND intentDigest = :intent AND selectionDigest = :selection AND fileIndex = :index AND registeredName = :name AND registryRevision = :revision AND registryEtag = :etag AND contentDigest = :content AND contentBytes = :bytes AND mountPath = :path AND mediaType = :media AND #mode = :mode)";
    let mut plan = TransactionPlan::new(format!("create-file-{selection}-{index}"));
    plan.put(
        PREPARED_FILE_PARTICIPANT,
        Put::builder()
            .table_name(table)
            .set_item(Some(item))
            .condition_expression(same_file)
            .expression_attribute_names("#mode", "mode")
            .expression_attribute_values(":workspace", s(prepared.workspace.to_string()))
            .expression_attribute_values(":receipt", s(prepared.receipt_key_sha256.clone()))
            .expression_attribute_values(":intent", s(prepared.intent.to_string()))
            .expression_attribute_values(":selection", s(selection))
            .expression_attribute_values(":index", n(index_u64))
            .expression_attribute_values(":name", s(file.name.to_string()))
            .expression_attribute_values(":revision", n(file.revision))
            .expression_attribute_values(":etag", s(file.etag.to_string()))
            .expression_attribute_values(":content", s(file.content.to_wire()))
            .expression_attribute_values(":bytes", n(file.size_bytes))
            .expression_attribute_values(":path", s(file.mount_path.as_str()))
            .expression_attribute_values(":media", s(file.media_type.clone()))
            .expression_attribute_values(":mode", s(file.mode.as_str())),
    )?;
    Ok(plan)
}

/// Atomically elects the prepared selection and a fresh physical root control.
///
/// No provider launch is legal before this two-action plan commits. An exact
/// replay by the same coordinator is idempotent; a different coordinator loses
/// the conditions and must not launch or recreate execution state.
///
/// # Errors
///
/// Returns [`CreatePreparationError`] for an invalid preparation or rejected
/// transaction shape.
pub fn elect_plan(
    table: &str,
    prepared: &CreatePreparation,
) -> Result<TransactionPlan, CreatePreparationError> {
    let summary = prepared.validate()?;
    let root_body = prepared
        .root_record
        .canonical_bytes()
        .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
    let partition = preparation_partition(prepared.workspace, &prepared.receipt_key_sha256);
    let selection = summary.selection_digest.to_wire();
    let coordinator = coordinator_text(prepared.coordinator);
    let reclaim_at = preparation_reclaim_epoch_seconds(prepared.prepared_at)?;
    let header = ItemBuilder::new(PREPARATION)
        .set(crate::attr::PK, s(partition))
        .set(crate::attr::SK, s("PREPARED"))
        .set("state", s("prepared"))
        .set("workspaceId", s(prepared.workspace.to_string()))
        .set("organizationId", s(prepared.organization.to_string()))
        .set("intentDigest", s(prepared.intent.to_string()))
        .set("receiptKeySha256", s(prepared.receipt_key_sha256.clone()))
        .set("coordinator", s(coordinator.clone()))
        .set("sessionId", s(prepared.session.to_string()))
        .set("rootAgentId", s(prepared.root_agent.to_string()))
        .set("generationId", s(prepared.generation.to_string()))
        .set("selectionDigest", s(selection.clone()))
        .set("selectedFileCount", n(summary.file_count as u64))
        .set("selectedFileBytes", n(summary.total_bytes))
        .set("rootEntryId", s(summary.root_entry_id.to_hex()))
        .set("rootRecord", b(root_body))
        .set("authorityDigest", s(summary.authority_digest.to_wire()))
        .set("runtimeDefinition", b(prepared.runtime_definition.clone()))
        .set("resolvedConfig", b(prepared.resolved_config.clone()))
        .set_opt("metadata", prepared.metadata.clone().map(b))
        .set("materializedAgents", n(prepared.materialized_agents))
        .set("accountRevision", n(prepared.account_revision))
        .set("preparedAt", stamp(prepared.prepared_at))
        .set("expiresAtEpochSeconds", crate::attr::n_i64(reclaim_at))
        .build();

    let control_key = crate::keys::agent_control(prepared.session, prepared.root_agent);
    let mut control = ItemBuilder::new(AGENT_CONTROL)
        .set(crate::attr::PK, s(control_key.pk))
        .set(crate::attr::SK, s(control_key.sk))
        .set("agentId", s(prepared.root_agent.to_string()))
        .set("sessionId", s(prepared.session.to_string()))
        .set("workspaceId", s(prepared.workspace.to_string()))
        .set("generationId", s(prepared.generation.to_string()))
        .set("createCoordinator", s(coordinator.clone()))
        .set("createSelectionDigest", s(selection.clone()))
        .set("status", s("preparing"))
        .set("revision", n(0))
        .set("journalTail", n(0))
        .set("hasJournal", boolean(false))
        .set("limitsRevision", n(root_limits(&prepared.root_record)?.0))
        .set("maxRunDurationMs", n(root_limits(&prepared.root_record)?.1))
        .set("fence", n(0))
        .set("cancelEpoch", n(0))
        .set("stopRequested", boolean(false))
        .set("depth", n(0))
        .set("createdAt", stamp(prepared.prepared_at))
        .set("updatedAt", stamp(prepared.prepared_at));
    let budget = root_budget(&prepared.root_record)?;
    for dimension in DIMENSIONS {
        control = control
            .set(limit_attribute(dimension), n(budget.get(dimension)))
            .set(reserved_attribute(dimension), n(0))
            .set(used_attribute(dimension), n(0));
    }

    let same_election = "attribute_not_exists(pk) OR (authorityDigest = :authority AND coordinator = :coordinator AND sessionId = :session AND rootAgentId = :agent AND generationId = :generation)";
    let same_control = "attribute_not_exists(pk) OR (createSelectionDigest = :selection AND createCoordinator = :coordinator AND sessionId = :session AND agentId = :agent AND generationId = :generation AND revision = :zero AND hasJournal = :false)";
    let mut plan = TransactionPlan::new(format!("create-elect-{}", prepared.intent));
    plan.put(
        PREPARATION_PARTICIPANT,
        Put::builder()
            .table_name(table)
            .set_item(Some(header))
            .condition_expression(same_election)
            .expression_attribute_values(":authority", s(summary.authority_digest.to_wire()))
            .expression_attribute_values(":coordinator", s(coordinator.clone()))
            .expression_attribute_values(":session", s(prepared.session.to_string()))
            .expression_attribute_values(":agent", s(prepared.root_agent.to_string()))
            .expression_attribute_values(":generation", s(prepared.generation.to_string())),
    )?;
    plan.put(
        Participant::AGENT_ROOT_CONTROL,
        Put::builder()
            .table_name(table)
            .set_item(Some(control.build()))
            .condition_expression(same_control)
            .expression_attribute_values(":selection", s(selection))
            .expression_attribute_values(":coordinator", s(coordinator))
            .expression_attribute_values(":session", s(prepared.session.to_string()))
            .expression_attribute_values(":agent", s(prepared.root_agent.to_string()))
            .expression_attribute_values(":generation", s(prepared.generation.to_string()))
            .expression_attribute_values(":zero", n(0))
            .expression_attribute_values(":false", boolean(false)),
    )?;
    Ok(plan)
}

/// Commits physical sequence-zero `AgentStarted` readiness after launch/upload.
///
/// # Errors
///
/// Returns [`CreatePreparationError`] when the record is not the exact elected
/// root fact or the shared transaction compiler rejects the conditional update
/// and immutable journal append.
pub fn root_started_plan(
    table: &str,
    prepared: &CreatePreparation,
    started: &RootStarted,
) -> Result<TransactionPlan, CreatePreparationError> {
    let summary = prepared.validate()?;
    let body = prepared
        .root_record
        .canonical_bytes()
        .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
    let entry_id = summary.root_entry_id.to_hex();
    let selection = summary.selection_digest.to_wire();
    let coordinator = coordinator_text(prepared.coordinator);
    let key = crate::keys::agent_control(prepared.session, prepared.root_agent);
    let update = Update::builder()
        .table_name(table)
        .set_key(Some(crate::plan::key(&key.pk, &key.sk)))
        .condition_expression(
            "revision = :zero AND hasJournal = :false AND generationId = :generation AND createSelectionDigest = :selection AND createCoordinator = :coordinator",
        )
        .update_expression(
            "SET revision = :one, journalTail = :tail, journalTailHash = :entry, hasJournal = :true, #status = :idle, updatedAt = :now",
        )
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":zero", n(0))
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":tail", n(0))
        .expression_attribute_values(":false", boolean(false))
        .expression_attribute_values(":true", boolean(true))
        .expression_attribute_values(":generation", s(prepared.generation.to_string()))
        .expression_attribute_values(":selection", s(selection))
        .expression_attribute_values(":coordinator", s(coordinator))
        .expression_attribute_values(":entry", s(entry_id.clone()))
        .expression_attribute_values(":idle", s("idle"))
        .expression_attribute_values(":now", stamp(started.occurred_at));

    let journal_key = crate::keys::journal(prepared.session, prepared.root_agent, 0);
    let journal = ItemBuilder::new(JOURNAL_ENTRY)
        .set(crate::attr::PK, s(journal_key.pk))
        .set(crate::attr::SK, s(journal_key.sk))
        .set("seq", n(0))
        .set("entryId", s(entry_id))
        .set("kind", s("agent_started"))
        .set("bodyInline", b(body.clone()))
        .set("bodyBytes", n(body.len() as u64))
        .set("occurredAt", stamp(started.occurred_at))
        .build();
    let mut plan = TransactionPlan::new(format!("create-root-start-{}", prepared.intent));
    plan.update(Participant::AGENT_ROOT_CONTROL, update)?;
    plan.put(
        Participant::AGENT_JOURNAL,
        Put::builder()
            .table_name(table)
            .set_item(Some(journal))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(plan)
}

/// Production authority for staging, electing, and strongly reloading a create winner.
#[derive(Debug, Clone)]
pub struct CreatePreparationStore {
    client: Client,
    table: String,
}

impl CreatePreparationStore {
    /// Binds the authority to the physical session-authority table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// Stages every selected revision and elects exactly one immutable winner.
    ///
    /// A conditional loss or ambiguous commit is resolved only by a strong
    /// read of the receipt-keyed header and its exact selection rows. The loser
    /// receives the winner's identities and must never launch its own generation.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] for invalid metadata, a store failure,
    /// or corrupt winner state.
    pub async fn stage_and_elect(
        &self,
        prepared: &CreatePreparation,
    ) -> Result<CreatePreparation, CreatePreparationError> {
        let plans = stage_plans(&self.table, prepared)?;
        futures::stream::iter(plans)
            .map(|plan| async move { self.commit(&plan).await })
            .buffer_unordered(16)
            .try_collect::<Vec<_>>()
            .await?;

        let plan = elect_plan(&self.table, prepared)?;
        match self.commit(&plan).await {
            Ok(()) => Ok(prepared.clone()),
            Err(error @ StoreError::PreconditionFailed { .. })
            | Err(error @ StoreError::CommitAmbiguous { .. })
            | Err(error @ StoreError::Contended) => {
                match self
                    .load(prepared.workspace, &prepared.receipt_key_sha256)
                    .await?
                {
                    Some(winner) => Ok(winner),
                    None => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Strongly loads one elected winner and all selection rows.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] for read failures or any incomplete,
    /// cross-tenant, non-canonical, or self-contradictory stored authority.
    pub async fn load(
        &self,
        workspace: WorkspaceId,
        receipt_key_sha256: &str,
    ) -> Result<Option<CreatePreparation>, CreatePreparationError> {
        if receipt_key_sha256.len() != 64
            || !receipt_key_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CreatePreparationError::ReplayKey);
        }
        let partition = preparation_partition(workspace, receipt_key_sha256);
        let header = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(crate::attr::PK, s(partition.clone()))
            .key(crate::attr::SK, s("PREPARED"))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?
            .item;
        let Some(header) = header else {
            return Ok(None);
        };
        let header_row = Row::bind(&header, PREPARATION).map_err(StoreError::from)?;
        assert_header_binding(&header_row, workspace, receipt_key_sha256)?;
        let selection = header_row
            .string("selectionDigest")
            .map_err(StoreError::from)?;
        let prefix = format!("FILE#{selection}#");
        let mut files = Vec::new();
        let mut start = None;
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
                .expression_attribute_names("#pk", crate::attr::PK)
                .expression_attribute_names("#sk", crate::attr::SK)
                .expression_attribute_values(":pk", s(partition.clone()))
                .expression_attribute_values(":prefix", s(prefix.clone()))
                .consistent_read(true)
                .scan_index_forward(true)
                .set_exclusive_start_key(start)
                .send()
                .await
                .map_err(|error| classify(&error, Idempotence::Read))?;
            files.extend(output.items.unwrap_or_default());
            start = output.last_evaluated_key;
            if start.is_none() {
                break;
            }
        }
        decode_elected_preparation(&header, &files, workspace, receipt_key_sha256).map(Some)
    }

    /// Commits and strongly verifies the elected root `AgentStarted` record.
    ///
    /// A conditional loss or ambiguous result is never retried blindly. The
    /// exact control and sequence-zero journal rows are read and accepted only
    /// when they prove the elected record already committed.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] when the append fails, remains
    /// unresolved, or the durable rows disagree with the election.
    pub async fn commit_root_started(
        &self,
        prepared: &CreatePreparation,
        started: RootStarted,
    ) -> Result<DurableRootStarted, CreatePreparationError> {
        let plan = root_started_plan(&self.table, prepared, &started)?;
        match self.commit(&plan).await {
            Ok(()) => self.load_root_started(prepared).await,
            Err(StoreError::PreconditionFailed { .. })
            | Err(StoreError::CommitAmbiguous { .. })
            | Err(StoreError::Contended) => self.load_root_started(prepared).await,
            Err(error) => Err(error.into()),
        }
    }

    /// Strongly reads the exact control and initial journal rows.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] when either row is absent or does
    /// not prove the elected `AgentStarted` transition.
    pub async fn load_root_started(
        &self,
        prepared: &CreatePreparation,
    ) -> Result<DurableRootStarted, CreatePreparationError> {
        let control_key = crate::keys::agent_control(prepared.session, prepared.root_agent);
        let journal_key = crate::keys::journal(prepared.session, prepared.root_agent, 0);
        let (control, journal) = futures::try_join!(
            self.get(&control_key.pk, &control_key.sk),
            self.get(&journal_key.pk, &journal_key.sk),
        )?;
        let control = control.ok_or_else(|| corrupt("the elected root control is absent"))?;
        let journal = journal.ok_or_else(|| corrupt("the root AgentStarted row is absent"))?;
        decode_root_started(&control, &journal, prepared)
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        Ok(self
            .client
            .get_item()
            .table_name(&self.table)
            .key(crate::attr::PK, s(pk))
            .key(crate::attr::SK, s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?
            .item)
    }

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        Resolution::TargetItem,
                    ));
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }
}

/// Verifies the exact post-append control and journal pair.
///
/// # Errors
///
/// Returns [`CreatePreparationError`] for any binding, state, position, body,
/// or digest disagreement.
pub fn decode_root_started(
    control: &Item,
    journal: &Item,
    prepared: &CreatePreparation,
) -> Result<DurableRootStarted, CreatePreparationError> {
    let summary = prepared.validate()?;
    let control = Row::bind(control, AGENT_CONTROL).map_err(StoreError::from)?;
    let expected_hash = summary.root_entry_id.to_hex();
    if control
        .id::<WorkspaceId>("workspaceId")
        .map_err(StoreError::from)?
        != prepared.workspace
        || control
            .id::<SessionId>("sessionId")
            .map_err(StoreError::from)?
            != prepared.session
        || control.id::<AgentId>("agentId").map_err(StoreError::from)? != prepared.root_agent
        || control
            .id::<GenerationId>("generationId")
            .map_err(StoreError::from)?
            != prepared.generation
        || control.string("status").map_err(StoreError::from)? != "idle"
        || !control.boolean("hasJournal").map_err(StoreError::from)?
        || control.u64("journalTail").map_err(StoreError::from)? != 0
        || control
            .string("journalTailHash")
            .map_err(StoreError::from)?
            != expected_hash
        || control
            .string("createSelectionDigest")
            .map_err(StoreError::from)?
            != summary.selection_digest.to_wire()
        || control
            .string("createCoordinator")
            .map_err(StoreError::from)?
            != prepared.coordinator.to_string()
    {
        return Err(corrupt(
            "the root control does not prove the elected sequence-zero append",
        ));
    }
    let revision = control.u64("revision").map_err(StoreError::from)?;
    if revision != 1 {
        return Err(corrupt(format!(
            "the initial root control revision is {revision}, not one"
        )));
    }

    let journal = Row::bind(journal, JOURNAL_ENTRY).map_err(StoreError::from)?;
    let canonical = prepared
        .root_record
        .canonical_bytes()
        .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
    if journal.u64("seq").map_err(StoreError::from)? != 0
        || journal.string("entryId").map_err(StoreError::from)? != expected_hash
        || journal.string("kind").map_err(StoreError::from)? != "agent_started"
        || journal.bytes("bodyInline").map_err(StoreError::from)? != canonical.as_slice()
        || journal.u64("bodyBytes").map_err(StoreError::from)?
            != u64::try_from(canonical.len())
                .map_err(|error| corrupt(format!("root body length does not fit u64: {error}")))?
    {
        return Err(corrupt(
            "the immutable sequence-zero journal row disagrees with the elected root record",
        ));
    }
    Ok(DurableRootStarted {
        revision,
        journal_tail: 0,
        journal_tail_hash: summary.root_entry_id.0,
        occurred_at: journal.timestamp("occurredAt").map_err(StoreError::from)?,
    })
}

/// Decodes a strongly read elected header and its exact ordered selection rows.
///
/// This pure boundary is public so adapter tests can prove that the physical
/// rows reconstruct the complete winner without a second volatile dependency
/// read.
///
/// # Errors
///
/// Returns [`CreatePreparationError`] for an incomplete, cross-boundary,
/// malformed, or digest-inconsistent row set.
pub fn decode_elected_preparation(
    header: &Item,
    file_items: &[Item],
    workspace: WorkspaceId,
    receipt_key_sha256: &str,
) -> Result<CreatePreparation, CreatePreparationError> {
    let row = Row::bind(header, PREPARATION).map_err(StoreError::from)?;
    assert_header_binding(&row, workspace, receipt_key_sha256)?;
    if row.string("state").map_err(StoreError::from)? != "prepared" {
        return Err(corrupt("the elected header is not in prepared state"));
    }
    let stored_intent = parse_intent(row.string("intentDigest").map_err(StoreError::from)?)?;
    let selection_text = row.string("selectionDigest").map_err(StoreError::from)?;
    let selection_digest = ContentHash::parse(selection_text)
        .map_err(|error| corrupt(format!("selectionDigest is malformed: {error}")))?;
    let expected_count =
        usize::try_from(row.u64("selectedFileCount").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("selectedFileCount does not fit usize: {error}")))?;
    if file_items.len() != expected_count {
        return Err(corrupt(format!(
            "the header names {expected_count} files but {} exact selection rows exist",
            file_items.len()
        )));
    }
    let mut files = Vec::with_capacity(file_items.len());
    for (expected_index, item) in file_items.iter().enumerate() {
        let file = Row::bind(item, PREPARED_FILE).map_err(StoreError::from)?;
        if file.string("workspaceId").map_err(StoreError::from)? != workspace.to_string()
            || file.string("receiptKeySha256").map_err(StoreError::from)? != receipt_key_sha256
            || file.string("intentDigest").map_err(StoreError::from)? != stored_intent.to_string()
            || file.string("selectionDigest").map_err(StoreError::from)? != selection_text
        {
            return Err(corrupt("a selected-file row has another authority binding"));
        }
        let index = usize::try_from(file.u64("fileIndex").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("fileIndex does not fit usize: {error}")))?;
        if index != expected_index {
            return Err(corrupt(format!(
                "selected-file rows are not complete and ordered: expected {expected_index}, found {index}"
            )));
        }
        let name = ResourceName::parse(file.string("registeredName").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("registeredName is malformed: {error}")))?;
        let etag = ETag::parse(file.string("registryEtag").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("registryEtag is malformed: {error}")))?;
        let content = ContentHash::parse(file.string("contentDigest").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("contentDigest is malformed: {error}")))?;
        let mount_path = FilePath::parse(file.string("mountPath").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("mountPath is malformed: {error}")))?;
        let mode = match file.string("mode").map_err(StoreError::from)? {
            "0644" => RegisteredFileMode::V0644,
            "0755" => RegisteredFileMode::V0755,
            other => return Err(corrupt(format!("mode `{other}` is not registered"))),
        };
        files.push(PreparedFile {
            name,
            revision: file.u64("registryRevision").map_err(StoreError::from)?,
            etag,
            content,
            size_bytes: file.u64("contentBytes").map_err(StoreError::from)?,
            mount_path,
            media_type: file
                .string("mediaType")
                .map_err(StoreError::from)?
                .to_owned(),
            mode,
        });
    }
    let coordinator = Uuid7::decode_suffix(
        row.string("coordinator")
            .map_err(StoreError::from)?
            .as_bytes(),
    )
    .map_err(|error| corrupt(format!("coordinator is malformed: {error}")))?;
    let root_record =
        aex_brain_domain::journal::decode(row.bytes("rootRecord").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("rootRecord is malformed: {error}")))?;
    let prepared = CreatePreparation {
        workspace,
        organization: row.id("organizationId").map_err(StoreError::from)?,
        intent: stored_intent,
        receipt_key_sha256: receipt_key_sha256.to_owned(),
        coordinator,
        session: row.id("sessionId").map_err(StoreError::from)?,
        root_agent: row.id("rootAgentId").map_err(StoreError::from)?,
        generation: row.id("generationId").map_err(StoreError::from)?,
        files,
        root_record,
        runtime_definition: row
            .bytes("runtimeDefinition")
            .map_err(StoreError::from)?
            .to_vec(),
        resolved_config: row
            .bytes("resolvedConfig")
            .map_err(StoreError::from)?
            .to_vec(),
        metadata: row
            .opt_bytes("metadata")
            .map_err(StoreError::from)?
            .map(|bytes| bytes.to_vec()),
        materialized_agents: row.u64("materializedAgents").map_err(StoreError::from)?,
        account_revision: row.u64("accountRevision").map_err(StoreError::from)?,
        prepared_at: row.timestamp("preparedAt").map_err(StoreError::from)?,
    };
    let summary = prepared.validate()?;
    let stored_authority =
        ContentHash::parse(row.string("authorityDigest").map_err(StoreError::from)?)
            .map_err(|error| corrupt(format!("authorityDigest is malformed: {error}")))?;
    if summary.selection_digest != selection_digest
        || summary.file_count != expected_count
        || summary.total_bytes != row.u64("selectedFileBytes").map_err(StoreError::from)?
        || summary.root_entry_id.to_hex() != row.string("rootEntryId").map_err(StoreError::from)?
        || summary.authority_digest != stored_authority
    {
        return Err(corrupt(
            "the elected header digests disagree with the strongly loaded facts",
        ));
    }
    Ok(prepared)
}

fn assert_header_binding(
    row: &Row<'_>,
    workspace: WorkspaceId,
    receipt_key_sha256: &str,
) -> Result<(), CreatePreparationError> {
    if row.string("workspaceId").map_err(StoreError::from)? != workspace.to_string()
        || row.string("receiptKeySha256").map_err(StoreError::from)? != receipt_key_sha256
    {
        return Err(corrupt(
            "the replay-keyed header carries another workspace or receipt key",
        ));
    }
    Ok(())
}

fn corrupt(reason: impl Into<String>) -> CreatePreparationError {
    CreatePreparationError::Corrupt {
        reason: reason.into(),
    }
}

fn parse_intent(text: &str) -> Result<IntentDigest, CreatePreparationError> {
    if text.len() != 64 {
        return Err(corrupt("intentDigest is not 64 lowercase hex characters"));
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let pair = &text[index * 2..index * 2 + 2];
        if pair
            .bytes()
            .any(|byte| !(byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(corrupt("intentDigest is not 64 lowercase hex characters"));
        }
        *slot = u8::from_str_radix(pair, 16)
            .map_err(|_| corrupt("intentDigest is not 64 lowercase hex characters"))?;
    }
    Ok(IntentDigest::from_bytes(bytes))
}

fn root_budget(record: &JournalRecord) -> Result<DimensionVector, CreatePreparationError> {
    match record {
        JournalRecord::AgentStarted { budget, .. } => Ok(*budget),
        _ => Err(CreatePreparationError::RootRecord {
            reason: "the first root record is not AgentStarted",
        }),
    }
}

fn root_limits(record: &JournalRecord) -> Result<(u64, u64), CreatePreparationError> {
    match record {
        JournalRecord::AgentStarted { config, .. } => {
            Ok((config.limits_revision, config.limits.max_run_duration_ms))
        }
        _ => Err(CreatePreparationError::RootRecord {
            reason: "the first root record is not AgentStarted",
        }),
    }
}

fn preparation_partition(workspace: WorkspaceId, receipt_key_sha256: &str) -> String {
    format!("CREATE#{workspace}#{receipt_key_sha256}")
}

fn preparation_reclaim_epoch_seconds(
    prepared_at: Timestamp,
) -> Result<i64, CreatePreparationError> {
    prepared_at
        .unix_millis()
        .checked_add(PREPARATION_RECLAIM_AFTER_MS)
        .map(|millis| millis.div_euclid(1_000))
        .ok_or_else(|| {
            CreatePreparationError::Canonical(
                "the private preparation reclaim instant overflows i64".to_owned(),
            )
        })
}

fn coordinator_text(coordinator: Uuid7) -> String {
    coordinator.to_string()
}

const fn limit_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "limitTotalChildrenCreated",
        Dimension::ProviderCalls => "limitProviderCalls",
        Dimension::HandsCalls => "limitHandsCalls",
        Dimension::CostMicroUsd => "limitCostMicroUsd",
        Dimension::ActiveChildren => "limitActiveChildren",
        Dimension::QueuedChildren => "limitQueuedChildren",
        Dimension::RetainedResultBytes => "limitRetainedResultBytes",
    }
}

const fn reserved_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "reservedTotalChildrenCreated",
        Dimension::ProviderCalls => "reservedProviderCalls",
        Dimension::HandsCalls => "reservedHandsCalls",
        Dimension::CostMicroUsd => "reservedCostMicroUsd",
        Dimension::ActiveChildren => "reservedActiveChildren",
        Dimension::QueuedChildren => "reservedQueuedChildren",
        Dimension::RetainedResultBytes => "reservedRetainedResultBytes",
    }
}

const fn used_attribute(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::TotalChildrenCreated => "usedTotalChildrenCreated",
        Dimension::ProviderCalls => "usedProviderCalls",
        Dimension::HandsCalls => "usedHandsCalls",
        Dimension::CostMicroUsd => "usedCostMicroUsd",
        Dimension::ActiveChildren => "usedActiveChildren",
        Dimension::QueuedChildren => "usedQueuedChildren",
        Dimension::RetainedResultBytes => "usedRetainedResultBytes",
    }
}
