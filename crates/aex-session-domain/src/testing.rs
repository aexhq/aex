//! Deterministic fixtures for this crate's own cases and its property suite.
//!
//! Every builder is pure and takes no clock: identical calls produce identical
//! values, which is what lets a property test shrink to a reproducible witness.
//! Nothing here is randomised and nothing reads the environment.

use std::num::NonZeroU64;

use aex_content_domain::ContentDigest;
use aex_internal_contracts::{RunId, journal::JournalEntryKind};
use aex_operation_domain::DeletionGuard;
use aex_wire::CanonicalJson;
use aex_wire::ids::{
    AgentId, GenerationId, MessageId, OrganizationId, PrefixedId, SessionId, ToolCallId, Uuid7,
    WorkspaceId,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;

use crate::agent::{AgentControl, AgentKind, AgentStatus, MaterializedState, OpenEffectSet};
use crate::approval::{ApprovalBinding, BindingField};
use crate::budget::BudgetGrant;
use crate::ids::{
    AgentRevision, CancellationEpoch, EntryIdentity, JournalSeq, SessionRevision, UsageClosureId,
};
use crate::journal::{AuthorityFact, JournalBody, JournalEntry};
use crate::lineage::Lineage;
use crate::message::{Message, MessageRole, MessageState};
use crate::run::{Run, RunOutcome, RunStatus};
use crate::session::{
    PinnedRuntime, ResolvedConfigAuthority, Session, SessionStatus, WorkAdmission,
};
use crate::terminal::TerminalAttempt;

/// A deterministic instant.
///
/// # Panics
///
/// Panics only if a fixture literal is out of range, which is a broken fixture
/// rather than a reachable condition.
#[must_use]
pub fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("fixture instants are in range")
}

/// A deterministic identifier of any prefixed kind.
#[must_use]
pub fn id<T: PrefixedId>(tag: u8) -> T {
    T::from_uuid7(Uuid7::compose(1_700_000_000_000, [tag; 10]))
}

/// A deterministic private execution identity.
#[must_use]
pub fn run_id(tag: u8) -> RunId {
    RunId::from_uuid7(Uuid7::compose(1_700_000_000_000, [tag; 10]))
}
/// A deterministic spend grant.
///
/// # Panics
///
/// Panics only if a fixture literal is out of range.
#[must_use]
pub fn budget() -> BudgetGrant {
    BudgetGrant {
        max_spend_cents: NonZeroU64::new(1_000).expect("non-zero"),
    }
}

/// The state a materialized agent starts from.
#[must_use]
pub fn materialized_state() -> MaterializedState {
    MaterializedState {
        budget: budget(),
        generation: None,
    }
}

/// A deterministic immutable generation definition for one session.
///
/// # Panics
///
/// Panics only if a fixture literal is out of range.
#[must_use]
pub fn pinned_runtime(
    session: SessionId,
    workspace: WorkspaceId,
    organization: OrganizationId,
) -> PinnedRuntime {
    use aex_runtime_control::generation::{
        HandsGeneration, ImageIdentifier, ImagePin, ImageVersion, LimitsRevision, NetworkPolicy,
        guest_root,
    };

    PinnedRuntime::new(
        session,
        workspace,
        organization,
        HandsGeneration {
            generation: id::<GenerationId>(7),
            session,
            workspace,
            organization,
            size: aex_wire::types::ComputeSize::Gb1,
            image: ImagePin {
                identifier: ImageIdentifier("aex-hands-1gb".to_owned()),
                version: ImageVersion("1".to_owned()),
                artifact_digest: aex_wire::ids::ContentHash::from_bytes([7; 32]),
                capabilities: Vec::new(),
            },
            network: NetworkPolicy::None,
            protocol_version: aex_internal_contracts::SchemaVersion::V1,
            limits_revision: LimitsRevision(1),
            root: guest_root(),
        },
    )
    .expect("a fixture pin names its own session")
}

/// An idle, live session.
///
/// # Panics
///
/// Panics if the compile-time canonical fixture and its derived projections
/// disagree. That is a test-authoring defect, not a runtime input path.
#[must_use]
pub fn session_fixture() -> Session {
    let id_value: SessionId = id(1);
    let workspace: WorkspaceId = id(2);
    let organization: OrganizationId = id(3);
    let resolved = ResolvedConfigAuthority::new(
        CanonicalJson::parse(
            r#"{
                "catalogRevision":"mc1_0000000000000000000000000000000000000000000000000000000000000000",
                "compute":{
                    "baseline":{"memoryMiB":1024,"vcpus":1.0},
                    "endpointBandwidthMBps":100,
                    "maxConcurrentConnections":64,
                    "maxDiskGiB":20,
                    "peak":{"memoryMiB":2048,"vcpus":2.0},
                    "size":"1gb"
                },
                "lifecycle":{
                    "idleSuspendAfterSeconds":180,
                    "maximumLifetimeSeconds":28800,
                    "resumeOnLiveFileAccess":true,
                    "resumeOnMessage":true
                },
                "model":"gpt-test",
                "network":{"hands":{"mode":"none"}},
                "packages":[],
                "provider":"openai",
                "providerCredentialId":"pcr_01h455vb4pex5vsknk084sn02q",
                "registered":{}
            }"#,
        )
        .expect("canonical fixture config"),
        ProviderId::Openai,
        "gpt-test".to_owned(),
    )
    .expect("matching fixture projections");
    let pinned_runtime = pinned_runtime(id_value, workspace, organization);
    let generation = pinned_runtime.generation();
    Session {
        id: id_value,
        workspace,
        organization,
        status: SessionStatus::Idle,
        lifecycle: crate::SessionLifecycle::launched(generation, moment(0))
            .expect("the fixture launch instant is representable"),
        revision: SessionRevision::INITIAL,
        active_run: None,
        work_admission: WorkAdmission::Open,
        cancellation: CancellationEpoch::INITIAL,
        deletion: DeletionGuard::live(id_value),
        mutation_guard: None,
        root_agent: id::<AgentId>(4),
        generation: Some(generation),
        pinned_runtime,
        provider_credential: crate::ProviderCredentialPin {
            credential: aex_wire::ids::ProviderCredentialId::from_uuid7(Uuid7::compose(
                1, [12; 10],
            )),
            provider: ProviderId::Openai,
            source_generation: 1,
            revision: 1,
        },
        lineage: Lineage::ROOT,
        resolved,
        metadata: None,
        created_at: moment(0),
        updated_at: moment(0),
    }
}

/// A subagent with an empty journal.
#[must_use]
pub fn child_agent() -> AgentControl {
    AgentControl {
        id: id::<AgentId>(5),
        session: id::<SessionId>(1),
        kind: AgentKind::Subagent,
        parent: Some(id::<AgentId>(4)),
        depth: 1,
        status: AgentStatus::Running,
        revision: AgentRevision::INITIAL,
        journal_tail: JournalSeq::INITIAL,
        last_entry: None,
        claim: None,
        join: None,
        budget: Some(budget()),
        open_effects: OpenEffectSet::new(),
        pending_approval: None,
        queue_reason: None,
        generation: None,
        terminal: None,
        created_at: moment(0),
    }
}

/// One journal entry at a position, carrying a given authority fact.
///
/// The identity is derived from the position and the fact discriminant, so two
/// calls with the same arguments produce the same entry and different arguments
/// produce different identities.
///
/// # Panics
///
/// Panics only if a fixture literal is out of range.
#[must_use]
pub fn entry_at(agent: AgentId, seq: u64, fact: AuthorityFact) -> JournalEntry {
    let mut identity = [0_u8; 32];
    identity[0..8].copy_from_slice(&seq.to_le_bytes());
    identity[8] = fact_tag(fact);
    JournalEntry {
        agent,
        seq: JournalSeq(seq),
        kind: JournalEntryKind::AssistantMessage,
        identity: EntryIdentity::from_bytes(identity),
        body: JournalBody::Inline(b"{}".to_vec()),
        fact,
        recorded_at: moment(i64::try_from(seq).expect("fixture positions are small")),
    }
}

const fn fact_tag(fact: AuthorityFact) -> u8 {
    match fact {
        AuthorityFact::None => 0,
        AuthorityFact::EffectOpened(_) => 1,
        AuthorityFact::EffectSettled(_) => 2,
        AuthorityFact::ApprovalRaised(_) => 3,
        AuthorityFact::ApprovalResolved(_) => 4,
        AuthorityFact::Terminal(_) => 5,
    }
}

/// A session with a running run, its agent and one open message.
///
/// # Panics
///
/// Panics only if a fixture literal is out of range.
#[must_use]
pub fn running_session() -> (Session, Run, AgentControl, Message) {
    let mut session = session_fixture();
    let run_id = run_id(6);
    let message_id = id::<MessageId>(7);
    session
        .lifecycle
        .admit_message(
            message_id,
            run_id,
            crate::ResolvedMessageBounds {
                max_spend_cents: NonZeroU64::new(1_000).expect("non-zero"),
                deadline: moment(60_000),
            },
            moment(1),
        )
        .expect("fixture message admission");
    session.active_run = Some(run_id);
    session.status = session.lifecycle.status;

    let agent = child_agent();
    let run = Run {
        id: run_id,
        session: session.id,
        message: message_id,
        status: RunStatus::Running,
        max_spend_cents: NonZeroU64::new(1_000).expect("non-zero"),
        deadline: moment(60_000),
        cancellation_at_admission: session.cancellation,
        queued_at: moment(0),
        started_at: Some(moment(1)),
        terminal_at: None,
        outcome: None,
        telemetry_complete: None,
        telemetry_gaps: None,
    };
    let message = Message {
        id: id::<MessageId>(8),
        session: session.id,
        run: Some(run_id),
        agent: agent.id,
        role: MessageRole::Assistant,
        state: MessageState::Open,
        parts: Vec::new(),
        created_at: moment(1),
        sealed_at: None,
    };
    (session, run, agent, message)
}

/// A terminal attempt that agrees with the session it names.
#[must_use]
pub fn terminal_attempt(session: &Session, run: &Run) -> TerminalAttempt {
    TerminalAttempt {
        run: run.id,
        outcome: RunOutcome::Succeeded {
            output_messages: Vec::new(),
        },
        at: moment(10),
        session_revision_seen: session.revision,
        cancellation_seen: session.cancellation,
        agent_fence: crate::ids::AgentFence::INITIAL,
        usage_closure: UsageClosureId(Uuid7::compose(1, [11; 10])),
    }
}

/// A binding whose eleven fields are all distinct fixture values.
#[must_use]
pub fn approval_binding() -> ApprovalBinding {
    ApprovalBinding {
        session: id::<SessionId>(1),
        run: run_id(6),
        agent: id::<AgentId>(5),
        tool_call: id::<ToolCallId>(12),
        tool: "write_file".to_owned(),
        argument_digest: ContentDigest::of(b"arguments"),
        implementation_digest: ContentDigest::of(b"implementation"),
        config_digest: ContentDigest::of(b"config"),
        expected_generation: Some(id::<GenerationId>(13)),
        expected_provider_credential_revision: 1,
        expected_config_revision: 4,
    }
}

/// The same binding with exactly one field changed.
#[must_use]
pub fn drift_field(binding: &ApprovalBinding, field: BindingField) -> ApprovalBinding {
    let mut drifted = binding.clone();
    match field {
        BindingField::Session => drifted.session = id::<SessionId>(90),
        BindingField::Run => drifted.run = run_id(91),
        BindingField::Agent => drifted.agent = id::<AgentId>(92),
        BindingField::ToolCall => drifted.tool_call = id::<ToolCallId>(93),
        BindingField::Tool => "read_file".clone_into(&mut drifted.tool),
        BindingField::ArgumentDigest => drifted.argument_digest = ContentDigest::of(b"other"),
        BindingField::ImplementationDigest => {
            drifted.implementation_digest = ContentDigest::of(b"other");
        }
        BindingField::ConfigDigest => drifted.config_digest = ContentDigest::of(b"other"),
        BindingField::ExpectedGeneration => {
            drifted.expected_generation = Some(id::<GenerationId>(94));
        }
        BindingField::ExpectedProviderCredentialRevision => {
            drifted.expected_provider_credential_revision = 99;
        }
        BindingField::ExpectedConfigRevision => drifted.expected_config_revision = 99,
    }
    drifted
}
