//! The declarative transaction plan.
//!
//! This is the stream's load-bearing cross-stream artifact. A use case returns a
//! [`SessionTransaction`] and **never commits**; the deployable submits it. That
//! is what makes "one command = one transaction, no external call inside it" a
//! unit-testable property rather than a review convention.
//!
//! The [`Condition`], [`Write`] and [`Hint`] vocabularies are **closed** (D-04).
//! An adapter maps each condition onto exactly one provider condition expression
//! and may not drop, weaken, merge or reorder one; each write carries its own
//! table family and the adapter routes by that alone; a hint is emitted strictly
//! after a successful commit and never influences it.

use std::collections::BTreeSet;

use aex_content_domain::{ContentDigest, ContentRoot, GrantId, Pin, RegistryKind};
use aex_operation_domain::{DeletionEpoch, DeletionState, Fence, Operation};
use aex_secret_domain::{
    CustodyRevision, OwnerKeyEdgeId, RevocationEpoch, SecretName, SessionCustody, WorkspaceSecret,
};
use aex_session_domain::{
    AccountRevision, AgentControl, AgentFence, AgentRevision, Approval, AuthorizationEpoch,
    CancellationEpoch, IdempotencyReceipt, JournalPage, JournalSeq, Message, OutboxEvent,
    ReservationId, Run, Session, SessionRevision, SessionTombstone, WorkAdmission,
};
use aex_wire::ids::{
    AgentId, OperationId, OrganizationId, RunId, SessionId, UploadId, WorkspaceId,
};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::{DownloadGrant, RegistryPointer, RegistrySelector, Upload, UploadState};

/// Largest number of actions one transaction may carry.
pub const MAX_ACTIONS: usize = 100;

/// Largest byte size one transaction may carry.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Which command produced a plan. Carried so an adapter can attribute a
/// provider failure without parsing the writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransactionIntent {
    /// Create a session.
    CreateSession,
    /// Admit a message and its run.
    AdmitMessage,
    /// Start an admitted run.
    StartRun,
    /// Settle a run.
    CommitTerminal,
    /// Stop a session's work.
    StopSession,
    /// Persist the live workspace.
    PersistWorkspace,
    /// Clone a session.
    CloneSession,
    /// Discard the live workspace.
    DiscardWorkspace,
    /// Rebind credentials.
    RebindCredentials,
    /// Trash a session.
    TrashSession,
    /// Restore a session.
    RestoreSession,
    /// Purge a session.
    PurgeSession,
    /// Decide an approval.
    RespondApproval,
    /// Advance a continued operation.
    ContinueOperation,
}

/// The identity of one item a write targets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    /// The table family the item lives in.
    pub family: TableFamily,
    /// The partition.
    pub partition: String,
    /// The sort position.
    pub sort: String,
}

/// Which regional table family a write belongs to.
///
/// The adapter routes by this alone, so adding a table never means teaching the
/// domain about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableFamily {
    /// The session authority.
    SessionAuthority,
    /// The durable work and operation records.
    WorkAuthority,
    /// Content descriptors, pins and owner edges.
    ContentAuthority,
    /// The named registry and uploads.
    Registry,
    /// Session credential custody.
    SecretCustody,
    /// Idempotency receipts.
    Idempotency,
    /// The native outbox.
    Outbox,
}

/// The stable identity of one condition inside a plan.
///
/// `CommitError::ConditionFailed` returns these, so the application maps a
/// provider rejection onto a typed customer error rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConditionId(pub u16);

/// One precondition the transaction commits under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The session head is at this revision.
    SessionRevision {
        /// Which session.
        session: SessionId,
        /// The expected revision.
        expected: SessionRevision,
    },
    /// The session is in one of these statuses.
    SessionStatusIn {
        /// Which session.
        session: SessionId,
        /// The allowed set.
        allowed: BTreeSet<aex_session_domain::SessionStatus>,
    },
    /// The session's active run is exactly this.
    SessionActiveRun {
        /// Which session.
        session: SessionId,
        /// The expected run.
        expected: Option<RunId>,
    },
    /// The session admits work in exactly this way.
    WorkAdmission {
        /// Which session.
        session: SessionId,
        /// The expected admission.
        expected: WorkAdmission,
    },
    /// The session's deletion state and epoch are exactly these.
    DeletionState {
        /// Which session.
        session: SessionId,
        /// The expected state.
        expected: DeletionState,
        /// The expected epoch.
        epoch: DeletionEpoch,
    },
    /// The session's cancellation epoch is exactly this.
    CancellationEpoch {
        /// Which session.
        session: SessionId,
        /// The expected epoch.
        expected: CancellationEpoch,
    },
    /// No whole-session command holds the exclusion.
    MutationGuardFree {
        /// Which session.
        session: SessionId,
    },
    /// This operation holds the exclusion.
    MutationGuardHeldBy {
        /// Which session.
        session: SessionId,
        /// The expected holder.
        holder: OperationId,
    },
    /// The agent control record is at this revision.
    AgentRevision {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The expected revision.
        expected: AgentRevision,
    },
    /// The agent's fence is at least this.
    AgentFence {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The floor.
        at_least: AgentFence,
    },
    /// The agent's journal tail is exactly this.
    JournalTail {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The expected tail.
        expected: JournalSeq,
    },
    /// The run has not settled.
    RunNonTerminal {
        /// The owning session, required to locate the physical run row.
        session: SessionId,
        /// Which run.
        run: RunId,
    },
    /// The account projection has reached at least this revision.
    AccountRevisionAtLeast {
        /// Which organization.
        organization: OrganizationId,
        /// The floor.
        at_least: AccountRevision,
    },
    /// The workspace authorization epoch has reached at least this.
    AuthorizationEpochAtLeast {
        /// Which workspace.
        workspace: WorkspaceId,
        /// The floor.
        at_least: AuthorizationEpoch,
    },
    /// The registry pointer carries exactly this tag.
    RegistryEtag {
        /// Which pointer.
        selector: RegistrySelector,
        /// The expected tag.
        expected: ETag,
    },
    /// The upload is in exactly this state.
    UploadState {
        /// Which upload.
        upload: UploadId,
        /// The expected state.
        expected: UploadState,
    },
    /// The reservation is still open.
    ReservationOpen {
        /// Which reservation.
        reservation: ReservationId,
    },
    /// The workspace owns this body.
    ContentOwned {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Which body.
        digest: ContentDigest,
    },
    /// A root pin exists.
    RootPinPresent {
        /// Which root.
        root: ContentRoot,
    },
    /// The grant has not lapsed.
    GrantUnexpired {
        /// Which grant.
        grant: GrantId,
        /// The instant to evaluate against.
        now: Timestamp,
    },
    /// The session's durable root and persist revision are exactly these.
    PersistRoot {
        /// Which session.
        session: SessionId,
        /// The expected root.
        expected: ContentRoot,
        /// The expected persist revision.
        revision: aex_session_domain::PersistRevision,
    },
    /// The work item's fence is exactly this.
    OperationFence {
        /// Which operation.
        operation: OperationId,
        /// The expected fence.
        fence: Fence,
    },
    /// The secret's revocation epoch is exactly this.
    SecretRevocationEpoch {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Which name.
        name: SecretName,
        /// The expected epoch.
        expected: RevocationEpoch,
    },
    /// The session's custody revision is exactly this.
    CustodyRevision {
        /// Which session.
        session: SessionId,
        /// The expected revision.
        expected: CustodyRevision,
    },
    /// The item does not exist.
    ItemAbsent(ItemKey),
    /// The item exists.
    ItemPresent(ItemKey),
}

impl Condition {
    /// The item family the condition reads, for adapter routing.
    #[must_use]
    pub const fn family(&self) -> TableFamily {
        match self {
            Self::SessionRevision { .. }
            | Self::SessionStatusIn { .. }
            | Self::SessionActiveRun { .. }
            | Self::WorkAdmission { .. }
            | Self::DeletionState { .. }
            | Self::CancellationEpoch { .. }
            | Self::MutationGuardFree { .. }
            | Self::MutationGuardHeldBy { .. }
            | Self::AgentRevision { .. }
            | Self::AgentFence { .. }
            | Self::JournalTail { .. }
            | Self::RunNonTerminal { .. }
            | Self::AccountRevisionAtLeast { .. }
            | Self::AuthorizationEpochAtLeast { .. }
            | Self::PersistRoot { .. } => TableFamily::SessionAuthority,
            Self::RegistryEtag { .. } | Self::UploadState { .. } => TableFamily::Registry,
            Self::ReservationOpen { .. } | Self::OperationFence { .. } => {
                TableFamily::WorkAuthority
            }
            Self::ContentOwned { .. }
            | Self::RootPinPresent { .. }
            | Self::GrantUnexpired { .. } => TableFamily::ContentAuthority,
            Self::SecretRevocationEpoch { .. } | Self::CustodyRevision { .. } => {
                TableFamily::SecretCustody
            }
            Self::ItemAbsent(key) | Self::ItemPresent(key) => key.family,
        }
    }

    /// The logical item this guard reads.
    ///
    /// Conditions sharing this identity are joined into one physical
    /// expression. If a write has the same identity, the expression belongs on
    /// that `Put`/`Update`/`Delete`; `DynamoDB` rejects a separate
    /// `ConditionCheck` against the same item.
    #[must_use]
    pub fn target(&self) -> ItemKey {
        let (partition, sort) = match self {
            Self::SessionRevision { session, .. }
            | Self::SessionStatusIn { session, .. }
            | Self::SessionActiveRun { session, .. }
            | Self::WorkAdmission { session, .. }
            | Self::DeletionState { session, .. }
            | Self::CancellationEpoch { session, .. }
            | Self::MutationGuardFree { session }
            | Self::MutationGuardHeldBy { session, .. }
            | Self::PersistRoot { session, .. } => (session.to_string(), "HEAD".to_owned()),
            Self::AgentRevision { session, agent, .. }
            | Self::AgentFence { session, agent, .. }
            | Self::JournalTail { session, agent, .. } => {
                (format!("{session}#{agent}"), "CONTROL".to_owned())
            }
            Self::RunNonTerminal { session, run } => (session.to_string(), format!("RUN#{run}")),
            Self::AccountRevisionAtLeast { organization, .. } => {
                (organization.to_string(), "ACCOUNT".to_owned())
            }
            Self::AuthorizationEpochAtLeast { workspace, .. } => {
                (workspace.to_string(), "AUTHORIZATION".to_owned())
            }
            Self::RegistryEtag { selector, .. } => (
                selector.workspace.to_string(),
                format!(
                    "REG#{}#{}",
                    registry_kind_tag(selector.kind),
                    selector.name.as_str()
                ),
            ),
            Self::UploadState { upload, .. } => (upload.to_string(), "UPLOAD".to_owned()),
            Self::ReservationOpen { reservation } => {
                (reservation.0.to_string(), "RESERVATION".to_owned())
            }
            Self::ContentOwned { workspace, digest } => {
                (workspace.to_string(), format!("CONTENT#{digest}"))
            }
            Self::RootPinPresent { root } => (format!("{:x?}", root.digest), "ROOT_PIN".to_owned()),
            Self::GrantUnexpired { grant, .. } => (grant.0.to_string(), "GRANT".to_owned()),
            Self::OperationFence { operation, .. } => {
                (operation.to_string(), "OPERATION".to_owned())
            }
            Self::SecretRevocationEpoch {
                workspace, name, ..
            } => (
                workspace.to_string(),
                format!("SECRET_REVOCATION#{}", name.as_str()),
            ),
            Self::CustodyRevision { session, .. } => (session.to_string(), "CUSTODY".to_owned()),
            Self::ItemAbsent(key) | Self::ItemPresent(key) => return key.clone(),
        };
        ItemKey {
            family: self.family(),
            partition,
            sort,
        }
    }
}

/// One durable change the transaction makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Replace the session head.
    PutSessionHead(Box<Session>),
    /// Replace a complete message authority row.
    PutMessage(Box<Message>),
    /// Replace a run record.
    PutRun(Box<Run>),
    /// Replace an agent control record.
    PutAgentControl(Box<AgentControl>),
    /// Append a journal page.
    AppendJournalPage {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// The complete immutable journal page.
        page: Box<JournalPage>,
    },
    /// Replace an approval.
    PutApproval(Box<Approval>),
    /// Write an idempotency receipt.
    PutIdempotencyReceipt(Box<IdempotencyReceipt>),
    /// Replace an operation record.
    PutOperation(Box<Operation>),
    /// Strip a purged session's operation result.
    RedactOperationResult(OperationId),
    /// Replace a durable work item.
    PutWorkItem(Box<aex_operation_domain::WorkItem>),
    /// Append a native outbox event.
    PutOutboxEvent(Box<OutboxEvent>),
    /// Write an owner edge.
    PutOwnerEdge(Box<aex_content_domain::OwnerEdge>),
    /// Add a pin.
    PutPin(Box<Pin>),
    /// Remove a pin.
    DeletePin(Box<Pin>),
    /// Replace a registry pointer.
    PutRegistryPointer(Box<RegistryPointer>),
    /// Replace an upload record.
    PutUpload(Box<Upload>),
    /// Write a download grant.
    PutGrant(Box<DownloadGrant>),
    /// Replace a session's credential custody.
    PutCustody(Box<SessionCustody>),
    /// Replace a workspace secret.
    PutSecret(Box<WorkspaceSecret>),
    /// Write a session tombstone.
    PutTombstone(Box<SessionTombstone>),
    /// Delete an item outright.
    DeleteItem(ItemKey),
}

impl Write {
    /// The item family the write targets.
    #[must_use]
    pub const fn family(&self) -> TableFamily {
        match self {
            Self::PutSessionHead(_)
            | Self::PutMessage(_)
            | Self::PutRun(_)
            | Self::PutAgentControl(_)
            | Self::AppendJournalPage { .. }
            | Self::PutApproval(_)
            | Self::PutTombstone(_) => TableFamily::SessionAuthority,
            Self::PutIdempotencyReceipt(_) => TableFamily::Idempotency,
            Self::PutOperation(_) | Self::RedactOperationResult(_) | Self::PutWorkItem(_) => {
                TableFamily::WorkAuthority
            }
            Self::PutOutboxEvent(_) => TableFamily::Outbox,
            Self::PutOwnerEdge(_) | Self::PutPin(_) | Self::DeletePin(_) | Self::PutGrant(_) => {
                TableFamily::ContentAuthority
            }
            Self::PutRegistryPointer(_) | Self::PutUpload(_) => TableFamily::Registry,
            Self::PutCustody(_) | Self::PutSecret(_) => TableFamily::SecretCustody,
            Self::DeleteItem(key) => key.family,
        }
    }

    /// The item the write targets, for duplicate detection.
    #[must_use]
    pub fn target(&self) -> ItemKey {
        let (partition, sort) = match self {
            Self::PutSessionHead(session) => (session.id.to_string(), "HEAD".to_owned()),
            Self::PutMessage(message) => (
                message.session.to_string(),
                format!("MESSAGE#{}", message.id),
            ),
            Self::PutRun(run) => (run.session.to_string(), format!("RUN#{}", run.id)),
            Self::PutAgentControl(agent) => (
                format!("{}#{}", agent.session, agent.id),
                "CONTROL".to_owned(),
            ),
            Self::AppendJournalPage { session, page } => (
                format!("{session}#{}", page.agent),
                format!("JOURNAL#{}", page.first.0),
            ),
            Self::PutApproval(approval) => (
                approval.binding.session.to_string(),
                format!("APPROVAL#{}", approval.id),
            ),
            Self::PutIdempotencyReceipt(receipt) => {
                (receipt.intent.to_string(), "RECEIPT".to_owned())
            }
            Self::PutOperation(operation) => (operation.id.to_string(), "OPERATION".to_owned()),
            Self::RedactOperationResult(id) => (id.to_string(), "OPERATION".to_owned()),
            Self::PutWorkItem(item) => (item.operation.to_string(), format!("WORK#{}", item.id.0)),
            Self::PutOutboxEvent(event) => {
                (event.session.to_string(), format!("OUTBOX#{}", event.run))
            }
            Self::PutOwnerEdge(edge) => {
                (edge.workspace.to_string(), format!("EDGE#{:?}", edge.pin))
            }
            Self::PutPin(pin) | Self::DeletePin(pin) => ("PIN".to_owned(), format!("{pin:?}")),
            Self::PutRegistryPointer(pointer) => (
                pointer.workspace.to_string(),
                format!(
                    "REG#{}#{}",
                    registry_kind_tag(pointer.kind),
                    pointer.name.as_str()
                ),
            ),
            Self::PutUpload(upload) => (upload.id.to_string(), "UPLOAD".to_owned()),
            Self::PutGrant(grant) => (grant.id.0.to_string(), "GRANT".to_owned()),
            Self::PutCustody(custody) => (custody.session.to_string(), "CUSTODY".to_owned()),
            Self::PutSecret(secret) => (
                secret.workspace.to_string(),
                format!("SECRET#{}", secret.name.as_str()),
            ),
            Self::PutTombstone(tombstone) => {
                (tombstone.session.to_string(), "TOMBSTONE".to_owned())
            }
            Self::DeleteItem(key) => return key.clone(),
        };
        ItemKey {
            family: self.family(),
            partition,
            sort,
        }
    }

    /// A conservative byte estimate, used only by the envelope check.
    #[must_use]
    pub fn estimated_bytes(&self) -> usize {
        match self {
            Self::AppendJournalPage { page, .. } => page
                .entries
                .iter()
                .map(|entry| {
                    usize::try_from(entry.body.inline_len().unwrap_or(32)).unwrap_or(usize::MAX)
                        + 128
                })
                .sum(),
            Self::PutMessage(message) => 256 + message.parts.len() * 256,
            _ => 512,
        }
    }
}

const fn registry_kind_tag(kind: RegistryKind) -> u8 {
    kind.discriminant()
}

/// One after-commit notification.
///
/// A hint is never truth and never a condition. It says "there may be work", and
/// the authority says what the work is; a lost hint is a latency problem, never
/// a correctness one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// An agent may be runnable.
    WakeAgent {
        /// Which agent.
        agent: AgentId,
        /// Why.
        reason: aex_internal_contracts::wake::WakeHint,
    },
    /// An operation may be due.
    OperationDue {
        /// Which operation.
        operation: OperationId,
        /// When.
        due_at: Timestamp,
    },
    /// Content lifecycle work may be due.
    ContentLifecycle {
        /// Which epoch.
        epoch: aex_content_domain::GcEpoch,
    },
    /// An owner key edge must be destroyed.
    DestroyKeyEdge {
        /// Which edge.
        edge: OwnerKeyEdgeId,
    },
}

/// One command's whole durable effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransaction {
    /// Which command produced it.
    pub intent: TransactionIntent,
    /// What must hold.
    pub conditions: Vec<Condition>,
    /// What changes.
    pub writes: Vec<Write>,
    /// What to notify afterwards.
    pub after_commit: Vec<Hint>,
}

/// What a validated plan looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanShape {
    /// How many distinct physical item actions it carries after guard merging.
    pub actions: usize,
    /// Its estimated byte size.
    pub bytes: usize,
}

/// Why a plan is not submittable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The plan carries too many actions.
    #[error("plan carries {actions} actions, above the maximum of {max}")]
    TooManyActions {
        /// How many.
        actions: usize,
        /// The maximum.
        max: usize,
    },
    /// The plan is too large.
    #[error("plan needs {bytes} bytes, above the maximum of {max}")]
    TooManyBytes {
        /// How many.
        bytes: usize,
        /// The maximum.
        max: usize,
    },
    /// A required condition is absent.
    #[error("plan is missing a required condition")]
    MissingRequiredCondition(ConditionId),
    /// Two writes target the same item.
    #[error("plan writes the same item twice")]
    DuplicateWriteTarget(Box<ItemKey>),
    /// A hint was placed where a condition belongs.
    #[error("a hint can never be a condition")]
    HintUsedAsCondition,
}

impl SessionTransaction {
    /// Whether the plan may be submitted.
    ///
    /// Fails rather than splitting (D-17): a silent split breaks "commits
    /// completely or not at all", which is the only reason the plan exists.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError`] for an over-large plan or a duplicate write target.
    pub fn validate(&self) -> Result<PlanShape, PlanError> {
        let mut action_targets = BTreeSet::new();
        action_targets.extend(self.conditions.iter().map(Condition::target));
        action_targets.extend(self.writes.iter().map(Write::target));
        let actions = action_targets.len();
        if actions > MAX_ACTIONS {
            return Err(PlanError::TooManyActions {
                actions,
                max: MAX_ACTIONS,
            });
        }
        let bytes: usize = self.writes.iter().map(Write::estimated_bytes).sum();
        if bytes > MAX_BYTES {
            return Err(PlanError::TooManyBytes {
                bytes,
                max: MAX_BYTES,
            });
        }
        let mut seen: BTreeSet<ItemKey> = BTreeSet::new();
        for write in &self.writes {
            let target = write.target();
            if !seen.insert(target.clone()) {
                return Err(PlanError::DuplicateWriteTarget(Box::new(target)));
            }
        }
        Ok(PlanShape { actions, bytes })
    }

    /// The stable identity of each condition, in plan order.
    #[must_use]
    pub fn condition_ids(&self) -> Vec<ConditionId> {
        (0..self.conditions.len())
            .map(|index| ConditionId(u16::try_from(index).unwrap_or(u16::MAX)))
            .collect()
    }
}

/// A projected result plus the one plan that would produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned<T> {
    /// The transaction the deployable submits.
    pub plan: SessionTransaction,
    /// What the caller is told, once the plan commits.
    pub projected: T,
}

#[cfg(test)]
mod tests {
    use super::{
        Condition, ItemKey, MAX_ACTIONS, PlanError, SessionTransaction, TableFamily,
        TransactionIntent, Write,
    };

    fn key(sort: &str) -> ItemKey {
        ItemKey {
            family: TableFamily::SessionAuthority,
            partition: "ses".to_owned(),
            sort: sort.to_owned(),
        }
    }

    fn plan(conditions: Vec<Condition>, writes: Vec<Write>) -> SessionTransaction {
        SessionTransaction {
            intent: TransactionIntent::CreateSession,
            conditions,
            writes,
            after_commit: Vec::new(),
        }
    }

    #[test]
    fn an_empty_plan_validates() {
        let shape = plan(Vec::new(), Vec::new()).validate().expect("validates");
        assert_eq!(shape.actions, 0);
        assert_eq!(shape.bytes, 0);
    }

    #[test]
    fn a_duplicate_write_target_is_rejected() {
        let outcome = plan(
            Vec::new(),
            vec![Write::DeleteItem(key("A")), Write::DeleteItem(key("A"))],
        )
        .validate();
        assert!(matches!(outcome, Err(PlanError::DuplicateWriteTarget(_))));
    }

    #[test]
    fn an_over_large_plan_fails_rather_than_splitting() {
        let writes: Vec<Write> = (0..=MAX_ACTIONS)
            .map(|index| Write::DeleteItem(key(&index.to_string())))
            .collect();
        assert!(matches!(
            plan(Vec::new(), writes).validate(),
            Err(PlanError::TooManyActions { .. })
        ));
    }

    #[test]
    fn condition_ids_are_positional_and_stable() {
        let value = plan(
            vec![
                Condition::MutationGuardFree {
                    session: aex_wire::ids::PrefixedId::from_uuid7(aex_wire::ids::Uuid7::compose(
                        1, [1; 10],
                    )),
                },
                Condition::ItemAbsent(key("A")),
            ],
            Vec::new(),
        );
        let ids = value.condition_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids, value.condition_ids());
    }

    #[test]
    fn guards_on_a_written_item_count_as_one_physical_action() {
        let session =
            aex_wire::ids::PrefixedId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
        let value = plan(
            vec![
                Condition::MutationGuardFree { session },
                Condition::SessionActiveRun {
                    session,
                    expected: None,
                },
            ],
            vec![Write::DeleteItem(ItemKey {
                family: TableFamily::SessionAuthority,
                partition: session.to_string(),
                sort: "HEAD".to_owned(),
            })],
        );
        assert_eq!(value.validate().expect("valid").actions, 1);
    }
}
