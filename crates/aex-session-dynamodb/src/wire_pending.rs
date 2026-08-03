//! Peer-owned vocabulary, defined here until the owning crate publishes it.
//!
//! Everything in this module belongs to `aex-session-app`, `aex-session-domain`,
//! `aex-operation-domain`, `aex-workspace-domain` or `aex-brain-domain`. It is
//! declared here so this adapter compiles and is tested ahead of its peers.
//!
//! # State of the peers
//!
//! All five peers have landed. The operation record now embeds its authoritative
//! domain envelope; the remaining local concepts either diverge from their peer
//! shape or were never published there. Every marker names the concrete mismatch
//! so reconciliation does not silently become a second authority.
//!
//! The non-negotiable property, whoever ends up owning these types, is that a
//! plan names a [`Participant`](crate::plan::Participant) per action. Without
//! that the cancellation decoding in [`crate::error`] cannot be exact, and a
//! caller receives an index instead of a reason.

use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::OrganizationId;
use aex_wire::ids::{
    AgentId, ApiKeyId, ApprovalId, ContentHash, GenerationId, ObservationId, ResourceName, RunId,
    SessionId, ToolCallId, WorkspaceId,
};
use aex_wire::types::Timestamp;

/// Descriptive workspace facts kept off the per-request placement row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceProfile {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Display name.
    pub name: String,
    /// URL-safe name.
    pub slug: String,
    /// When the workspace was created.
    pub created_at: Timestamp,
}

/// One durable effective limit from the regional capacity authority.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedWorkspaceLimit {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Which registered limit.
    pub id: aex_wire::limits::LimitId,
    /// The effective typed value, never an inferred default.
    pub effective_value: aex_wire::models::LimitValue,
    /// Whether the authority selected the shared default or an override.
    pub source: aex_wire::models::LimitSource,
    /// Monotonic concurrency token.
    pub revision: u64,
    /// When this effective record changed.
    pub changed_at: Timestamp,
}

/// Revision fence for one complete effective-limit projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedLimitBundleHead {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Authority revision shared by every row and the bundle payload.
    pub revision: u64,
    /// Canonical defaults revision used by the authority.
    pub defaults_revision: u64,
    /// When this effective set changed.
    pub changed_at: Timestamp,
}

/// One complete, revision-bound effective-limit payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedLimitBundle {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Authority revision that must equal the strong head read.
    pub revision: u64,
    /// Every registered limit, in registry order.
    pub limits: Vec<aex_wire::models::EffectiveWorkspaceLimit>,
}

// TODO(cross-stream): `aex-content-domain` publishes no body enum. The inline-or-stored
// question is `aex_content_domain::placement::Placement`, and the whole record is
// `aex_content_domain::descriptor::ContentDescriptor`, which is workspace-scoped and
// carries a ciphertext identity this adapter does not model.
/// A body that is either small enough to live inline or lives in content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// The sealed canonical plaintext, at most the inline ceiling.
    Inline(Vec<u8>),
    /// A `sha256:<64 lowercase hex>` reference into `regional-content`.
    Digest(String),
}

// TODO(cross-stream): `aex-session-domain` publishes no session-event type at all. The
// feed shape below is this adapter's own until it does.
/// One native event, which is also one outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    /// The workspace that owns the session and workspace event index row.
    pub workspace: WorkspaceId,
    /// The contiguous per-session sequence.
    pub event_seq: u64,
    /// The `obs_`-prefixed observation identity.
    ///
    /// The observation stream reads this table as the one authority for events
    /// rather than keeping a mirror, so the id it will publish is minted here.
    pub event_id: ObservationId,
    /// The event type.
    pub event_type: String,
    /// The run it belongs to.
    pub run: Option<RunId>,
    /// The agent it belongs to.
    pub agent: Option<AgentId>,
    /// The body.
    pub body: Body,
    /// When it happened.
    ///
    /// Monotone with `event_seq`: a later sequence never carries an earlier
    /// instant, which is what lets a reader order by either.
    pub occurred_at: Timestamp,
    /// Whether the outbox has delivered it.
    pub outbox_state: &'static str,
}

// TODO(cross-stream): the marker named the wrong crate. `aex-brain-domain` has no `agent`
// module; the peer type is `aex_session_domain::agent::AgentControl`, which carries an
// `AgentKind`, a typed `AgentStatus`, an `AgentRevision`, the journal tail's
// `EntryIdentity` and an `AgentClaim`.
/// One agent's control item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControl {
    /// Which agent.
    pub agent: AgentId,
    /// Its session.
    pub session: SessionId,
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// The Hands generation it is bound to.
    pub generation: GenerationId,
    /// Its optimistic revision.
    pub revision: u64,
    /// The highest journal sequence committed.
    pub journal_tail: u64,
    /// The canonical entry identity at `journal_tail`, absent before the first append.
    pub journal_tail_hash: Option<String>,
    /// The current claim owner, when claimed.
    pub claim_owner: Option<String>,
    /// The lease expiry, when claimed.
    pub lease_expires_at: Option<Timestamp>,
    /// The monotonic claim fence.
    pub fence: u64,
    /// How much of the hierarchical child budget is left.
    pub child_budget_remaining: u64,
    /// How much was granted.
    pub child_budget_granted: u64,
    /// Its status.
    pub status: String,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

// TODO(cross-stream): `aex_session_domain::journal::JournalEntry` exists and is keyed by
// agent, with a typed kind, a content-derived `EntryIdentity`, a `JournalBody` and an
// `AuthorityFact`. It carries neither a string entry id nor a byte count.
/// One immutable journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Its contiguous sequence.
    pub seq: u64,
    /// Its identity.
    pub entry_id: String,
    /// Its kind, from the closed generated `JournalEntryKind`.
    pub kind: String,
    /// Its body.
    pub body: Body,
    /// Its canonical length.
    pub body_bytes: u64,
    /// When it happened.
    pub occurred_at: Timestamp,
}

/// One durable operation record plus its optimistic store version.
///
/// The domain envelope is stored whole. Keeping a second, stringly operation
/// shape here previously discarded progress, typed results, failures and
/// lifecycle timestamps before a public reader could project them.
#[cfg(feature = "session-authority")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOperation {
    /// The authoritative domain envelope.
    pub record: aex_operation_domain::Operation,
    /// Its optimistic version.
    pub version: u64,
}

// TODO(cross-stream): `aex-workspace-domain` publishes no placement guard. Its modules are
// `grant`, `persist`, `registry` and `upload`, and none of them names workspace placement.
/// The projected authorization facts an admission fences on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementGuard {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The key epoch the assertion carried.
    pub key_epoch: u64,
    /// The account epoch the assertion carried.
    pub account_epoch: u64,
    /// The revocation epoch the assertion carried.
    pub revocation_epoch: u64,
}

// TODO(cross-stream): `aex-session-app` publishes no wake intent. Its ports module holds
// readers and an `AuthorityCommitter`; every write is expressed as an
// `aex_session_app::plan::SessionTransaction`.
/// The runnable continuation an admission or decision creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeIntent {
    /// The work identity.
    pub work_id: String,
    /// Its kind, from the closed `regional-work` vocabulary.
    pub kind: String,
    /// The hashed dedupe key that makes "one wake outstanding" durable.
    pub dedupe_key_sha256: String,
    /// When it becomes due.
    pub due_at: Timestamp,
    /// Its priority band.
    pub priority: u8,
}

// TODO(cross-stream): `aex-session-app` publishes no replay intent. Idempotent replay is
// `aex_session_domain::idempotency::ReplayDecision` over an `IdempotencyReceipt`, which is
// a domain decision rather than an adapter plan.
/// The replay identity a mutating command commits under.
#[derive(Debug, Clone)]
pub struct ReplayIntent {
    /// The workspace the receipt is partitioned under.
    pub workspace: WorkspaceId,
    /// The rendered scope.
    pub scope: String,
    /// The caller-chosen key.
    pub key: IdempotencyKey,
    /// What the request asked for.
    pub intent: IntentDigest,
    /// Which response shape the stored body decodes as.
    pub response_kind: String,
    /// The canonical response to store.
    pub response: Vec<u8>,
    /// When the receipt stops being readable.
    pub expires_at: Timestamp,
}

impl PartialEq for ReplayIntent {
    fn eq(&self, other: &Self) -> bool {
        self.workspace == other.workspace
            && self.scope == other.scope
            && self.key == other.key
            && self.intent == other.intent
            && self.response_kind == other.response_kind
            && self.response == other.response
            && self.expires_at == other.expires_at
    }
}

impl Eq for ReplayIntent {}

// TODO(cross-stream): `aex-brain-domain` has no `decision` module. Its planning vocabulary
// is `aex_brain_domain::planner::OwedStep` under an `aex_brain_domain::planner::PlanPolicy`.
/// One agent decision, planned by Brain and compiled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDecisionPlan {
    /// The session.
    pub session: SessionId,
    /// The agent.
    pub agent: AgentId,
    /// The session head facts the decision fences on.
    pub head_guard: HeadGuard,
    /// The control as the activation read it.
    pub control: AgentControl,
    /// The next status.
    pub next_status: String,
    /// The renewed lease.
    pub lease_expires_at: Timestamp,
    /// The journal entry being appended.
    pub entry: JournalEntry,
    /// The effect being prepared or settled, when there is one.
    pub effect: Option<EffectIntent>,
    /// The bounded preview event, when there is one.
    pub event: Option<SessionEvent>,
    /// The next wake, when the agent stays runnable.
    pub wake: Option<WakeIntent>,
    /// The request clock.
    pub now: Timestamp,
}

// TODO(cross-stream): the peer type is `aex_session_domain::session::MutationGuard`, taken
// and released by `session::acquire_mutation_guard` and `session::release_mutation_guard`,
// and it is held by an `OperationId` rather than by a revision.
/// The head facts a decision fences on without touching the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadGuard {
    /// The cancellation epoch.
    pub cancel_epoch: u64,
    /// The deletion epoch.
    pub deletion_epoch: u64,
}

// TODO(cross-stream): `aex-brain-domain` publishes no effect intent. A prepared effect is
// an `aex_brain_domain::effect::DurableEffect` carrying its `EffectKind`, `EffectClass`,
// request hash and `EffectState`.
/// A prepared or settled effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIntent {
    /// Its identity.
    pub effect_id: String,
    /// Its kind.
    pub kind: String,
    /// The hash of the request it dispatches.
    pub request_hash: String,
    /// Which attempt this is.
    pub attempt: u64,
    /// Whether this action prepares or settles it.
    pub stage: EffectStage,
}

// TODO(cross-stream): `aex-brain-domain` publishes no `EffectStage`. Where an effect
// stands is `aex_brain_domain::effect::EffectState`; how far a transport attempt got is
// the separate `aex_brain_domain::effect::DispatchStage`. The two are deliberately not one
// enum.
/// Whether an effect action prepares or settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectStage {
    /// First write; immutable identity.
    Prepare,
    /// Second write; conditions on the prepared request hash.
    Settle {
        /// The terminal state.
        state: &'static str,
    },
}

// TODO(cross-stream): `aex-brain-domain` has no `fanout` module. Child bookkeeping lives in
// `aex_brain_domain::child`, which publishes no paging plan.
/// One bounded page of a hierarchical fanout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutPagePlan {
    /// The session.
    pub session: SessionId,
    /// The parent agent.
    pub parent: AgentId,
    /// The parent's revision.
    pub parent_revision: u64,
    /// The fanout intent identity.
    pub intent_id: String,
    /// Which page of the intent this is.
    pub page: u32,
    /// The children this page materializes.
    pub children: Vec<ChildAgent>,
    /// The request clock.
    pub now: Timestamp,
}

// TODO(cross-stream): the nearest peer type is `aex_brain_domain::child::ChildRecord`,
// which carries a `ChildState` and a `ChildOutcome` rather than a stored status string.
/// One child agent created by a fanout page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildAgent {
    /// Its identity.
    pub agent: AgentId,
    /// The generation it is bound to.
    pub generation: GenerationId,
    /// The budget carved from the parent.
    pub child_budget: u64,
    /// Its wake.
    pub wake: WakeIntent,
}

// TODO(cross-stream): `aex-workspace-domain` publishes no workspace placement. Placement of
// *content* is `aex_content_domain::placement::Placement`, which is a different question.
/// The projected placement of one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePlacement {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Its organization.
    pub organization: OrganizationId,
    /// Its plane.
    pub plane: String,
    /// Its region.
    pub region: String,
    /// Whether it may execute.
    pub status: String,
    /// The current key epoch.
    pub key_epoch: u64,
    /// The current account epoch.
    pub account_epoch: u64,
    /// The current revocation epoch.
    pub revocation_epoch: u64,
    /// The signed feed position this row was written at.
    pub feed_sequence: u64,
    /// When it was written.
    pub updated_at: Timestamp,
}

// TODO(cross-stream): `aex-workspace-domain` publishes no key revocation. Secret revocation
// is `aex_secret_domain::revocation::RevocationEpoch`, which this adapter does not link.
/// One revoked workspace API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRevocation {
    /// Which key.
    pub api_key: ApiKeyId,
    /// When it was revoked.
    pub revoked_at: Timestamp,
    /// The epoch the revocation was published at.
    pub revoked_epoch: u64,
}

// TODO(cross-stream): `aex-workspace-domain` publishes no feed frontier.
/// How far the projection has been proved to be current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedFrontier {
    /// The signed sequence.
    pub sequence: u64,
    /// When it was signed.
    pub signed_at: Timestamp,
    /// Which key signed it.
    pub signature_key_id: String,
    /// The instant the signature covers through.
    pub covered_through: Timestamp,
}

// TODO(cross-stream): the record is `aex_session_domain::approval::Approval`. The
// shapes below now mirror it field for field — one `ApprovalBinding` of eleven bound
// fields, a four-arm status and a seven-arm cancel cause — so adopting the peer type
// is an import plus two newtype conversions rather than a decode change. Two values
// are held as `u64` here because the peer types (`aex_secret_domain::CustodyRevision`
// and the configuration revision) belong to crates this adapter deliberately does not
// link.
/// The exact call one approval authorizes.
///
/// All eleven bound fields are persisted and published.
/// `respond` revalidates the whole binding at decision time, so a row that stored
/// only the public subset could not tell an approver's binding from the current
/// one — and failing closed on drift is the entire point of the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalBinding {
    /// The session.
    pub session: SessionId,
    /// The run.
    pub run: RunId,
    /// The agent.
    pub agent: AgentId,
    /// The tool call.
    pub tool_call: ToolCallId,
    /// The canonical tool name.
    pub tool: ResourceName,
    /// The canonical argument digest.
    pub argument_digest: ContentHash,
    /// The tool implementation digest.
    pub implementation_digest: ContentHash,
    /// The resolved configuration digest.
    pub config_digest: ContentHash,
    /// The workspace generation the call expects.
    pub expected_generation: Option<GenerationId>,
    /// The custody revision the call expects.
    pub expected_custody: u64,
    /// The configuration revision the call expects.
    pub expected_config_revision: u64,
}

/// Where an approval is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalStatus {
    /// Waiting for a decision.
    Pending,
    /// Approved; the bound call may run.
    Approved,
    /// Denied; the bound call will not run.
    Denied,
    /// Withdrawn without a decision.
    Cancelled,
    /// Its explicit decision deadline elapsed.
    Expired,
}

impl ApprovalStatus {
    /// The stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    /// Parses a stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether no transition leaves this status.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// Why a pending approval was withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalCancelCause {
    /// A stop operation asked for it.
    StopRequested,
    /// The run was cancelled.
    RunCancelled,
    /// The session is being trashed.
    SessionTrashing,
    /// The account is paused.
    AccountPaused,
    /// The workspace generation is gone.
    ContinuityLost,
    /// The bound tool call itself was cancelled.
    ToolCallCancelled,
    /// The binding drifted between request and decision.
    BindingDrift,
}

impl ApprovalCancelCause {
    /// The stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StopRequested => "stop_requested",
            Self::RunCancelled => "run_cancelled",
            Self::SessionTrashing => "session_trashing",
            Self::AccountPaused => "account_paused",
            Self::ContinuityLost => "continuity_lost",
            Self::ToolCallCancelled => "tool_call_cancelled",
            Self::BindingDrift => "binding_drift",
        }
    }

    /// Parses a stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "stop_requested" => Some(Self::StopRequested),
            "run_cancelled" => Some(Self::RunCancelled),
            "session_trashing" => Some(Self::SessionTrashing),
            "account_paused" => Some(Self::AccountPaused),
            "continuity_lost" => Some(Self::ContinuityLost),
            "tool_call_cancelled" => Some(Self::ToolCallCancelled),
            "binding_drift" => Some(Self::BindingDrift),
            _ => None,
        }
    }
}

/// One approval, as the store holds it.
///
/// `workspace` is carried so the post-read tenancy check has something to
/// compare, exactly as every other row here does; `expires_at` is carried
/// because "when it stops being decidable" is a fact about the raised approval
/// and cannot be recomputed by a reader that holds only the approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// Which approval.
    pub approval: ApprovalId,
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// The exact call it authorizes, including its session.
    pub binding: ApprovalBinding,
    /// Its status.
    pub status: ApprovalStatus,
    /// Why it was withdrawn, when it was.
    pub cancel_cause: Option<ApprovalCancelCause>,
    /// When it was raised.
    pub created_at: Timestamp,
    /// When it stops being decidable.
    pub expires_at: Timestamp,
    /// When it settled.
    pub resolved_at: Option<Timestamp>,
}
