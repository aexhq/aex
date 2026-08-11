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
use aws_sdk_dynamodb::types::{Put, Update};
use serde::{Deserialize, Serialize};

use crate::attr::{ItemBuilder, b, boolean, n, s, stamp};
use crate::error::StoreError;
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
}

/// Provider-authoritative time at which readiness was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootStarted {
    /// When the exact sequence-zero fact became durable.
    pub occurred_at: Timestamp,
}

/// Why create preparation cannot be persisted or published.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CreatePreparationError {
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
}

impl CreatePreparation {
    /// Validates the full pre-launch boundary and derives immutable identities.
    ///
    /// # Errors
    ///
    /// Returns [`CreatePreparationError`] before any provider effect when the
    /// selected count/bytes, uniqueness or root generation is invalid.
    pub fn validate(&self) -> Result<PreparationSummary, CreatePreparationError> {
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
        let root_entry_id = self
            .root_record
            .content_hash()
            .map_err(|error| CreatePreparationError::Canonical(error.to_string()))?;
        Ok(PreparationSummary {
            file_count: self.files.len(),
            total_bytes,
            selection_digest: ContentHash::of(selection.as_bytes()),
            root_entry_id,
        })
    }
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
    let partition = preparation_partition(prepared.workspace, prepared.intent);
    let selection = summary.selection_digest.to_wire();
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
        .set("selectionDigest", s(selection.clone()))
        .set("fileIndex", n(index_u64))
        .set("registeredName", s(file.name.to_string()))
        .set("registryRevision", n(file.revision))
        .set("registryEtag", s(file.etag.to_string()))
        .set("contentDigest", s(file.content.to_wire()))
        .set("contentBytes", n(file.size_bytes))
        .set("mountPath", s(file.mount_path.as_str()))
        .set("mode", s(file.mode.as_str()))
        .set("preparedAt", stamp(prepared.prepared_at))
        .build();
    let mut plan = TransactionPlan::new(format!("create-file-{selection}-{index}"));
    plan.put(
        PREPARED_FILE_PARTICIPANT,
        Put::builder()
            .table_name(table)
            .set_item(Some(item))
            .condition_expression(
                "attribute_not_exists(pk) OR (selectionDigest = :selection AND fileIndex = :index)",
            )
            .expression_attribute_values(":selection", s(selection))
            .expression_attribute_values(":index", n(index_u64)),
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
    let partition = preparation_partition(prepared.workspace, prepared.intent);
    let selection = summary.selection_digest.to_wire();
    let coordinator = coordinator_text(prepared.coordinator);
    let header = ItemBuilder::new(PREPARATION)
        .set(crate::attr::PK, s(partition))
        .set(crate::attr::SK, s("PREPARED"))
        .set("state", s("prepared"))
        .set("workspaceId", s(prepared.workspace.to_string()))
        .set("organizationId", s(prepared.organization.to_string()))
        .set("intentDigest", s(prepared.intent.to_string()))
        .set("coordinator", s(coordinator.clone()))
        .set("sessionId", s(prepared.session.to_string()))
        .set("rootAgentId", s(prepared.root_agent.to_string()))
        .set("generationId", s(prepared.generation.to_string()))
        .set("selectionDigest", s(selection.clone()))
        .set("selectedFileCount", n(summary.file_count as u64))
        .set("selectedFileBytes", n(summary.total_bytes))
        .set("rootEntryId", s(summary.root_entry_id.to_hex()))
        .set("rootRecord", b(root_body))
        .set("preparedAt", stamp(prepared.prepared_at))
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

    let same_election = "attribute_not_exists(pk) OR (selectionDigest = :selection AND coordinator = :coordinator AND sessionId = :session AND rootAgentId = :agent AND generationId = :generation)";
    let same_control = "attribute_not_exists(pk) OR (createSelectionDigest = :selection AND createCoordinator = :coordinator AND sessionId = :session AND agentId = :agent AND generationId = :generation AND revision = :zero AND hasJournal = :false)";
    let mut plan = TransactionPlan::new(format!("create-elect-{}", prepared.intent));
    plan.put(
        PREPARATION_PARTICIPANT,
        Put::builder()
            .table_name(table)
            .set_item(Some(header))
            .condition_expression(same_election)
            .expression_attribute_values(":selection", s(selection.clone()))
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

fn root_budget(record: &JournalRecord) -> Result<DimensionVector, CreatePreparationError> {
    match record {
        JournalRecord::AgentStarted { budget, .. } => Ok(*budget),
        _ => Err(CreatePreparationError::RootRecord {
            reason: "the first root record is not AgentStarted",
        }),
    }
}

fn preparation_partition(workspace: WorkspaceId, intent: IntentDigest) -> String {
    format!("CREATE#{workspace}#{intent}")
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
