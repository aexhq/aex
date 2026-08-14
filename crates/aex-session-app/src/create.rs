//! Session admission.
//!
//! Session creation has a private preparation/election phase and a public
//! readiness publication phase. The public head and exact response receipt do
//! not exist until the exact provider generation is launched, every selected
//! workspace-file revision is materialized, and the root `AgentStarted` fact
//! is durable.
//!
//! # What this use case deliberately does not do
//!
//! **It reads the receipt before volatile planning.** An exact retry therefore
//! returns the winner's bytes even after a credential is revoked, a registry
//! file moves or an account pauses. The private claim elects concurrent first
//! attempts; the conditional final receipt closes publication races.
//!
//! The serving adapter must durably elect and reload the prepared identities
//! before making provider or guest effects. A concurrent loser must reuse that
//! claim; it must never mint and launch a second generation. This module keeps
//! preparation and publication separate so an unready session cannot be
//! projected accidentally.

use std::collections::BTreeMap;

use aex_content_domain::RegistryKind;
use aex_session_domain::{
    AgentRevision, CommandClass, DeletionGuard, IdempotencyIdentity, IdempotencyReceipt,
    JournalSeq, PinnedRuntime, ProviderCredentialPin, ReceiptKey, ReceiptOutcome, ReplayDecision,
    ResolvedConfigAuthority, ResourceId, ResourceKind, ResponseBody, Session, SessionLifecycle,
    SessionMetadata, SessionRevision, SessionStatus, WorkAdmission, pause_gate, replay,
};
use aex_wire::canonical::{CanonicalJson, to_jcs_string};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{AgentId, GenerationId, PrefixedId as _, ResourceName, SessionId, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models;
use aex_wire::types::ComputeSize;
use aex_workspace_domain::RegistrySelector;

use crate::error::AppError;
use crate::plan::{
    Condition, ItemKey, Planned, SessionTransaction, TableFamily, TransactionIntent, Write,
};
use crate::ports::AppContext;

/// The idempotency scope every create receipt is filed under.
///
/// The one subjectless base: a create names no resource yet, so the caller's
/// key is the whole identity.
pub const CREATE_SCOPE: &str = "session.create";

/// Synchronous creation's non-adjustable transport-honesty ceiling.
///
/// The effective revisioned workspace limit may lower this value, but may not
/// raise it: the smallest supported endpoint needs to finish launch and the
/// complete initial transfer within the serving request budget.
pub const INITIAL_FILES_HARD_MAX_BYTES: u64 = 536_870_912;

/// Admit one session.
///
/// Not `Eq`: the request carries `MetadataValue`, whose numeric arm is an
/// `f64`. Deriving a total equality over a float would be a lie, and nothing
/// here needs one.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateSession {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which organization pays.
    pub organization: aex_wire::ids::OrganizationId,
    /// The replay envelope the request arrived under.
    pub identity: IdempotencyIdentity,
    /// Exactly what the caller asked for.
    pub request: models::SessionCreateRequest,
}

/// One selected registered file, bound to the exact revision create resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedInitialFile {
    /// The registered name selected by the request.
    pub name: ResourceName,
    /// The exact registry revision.
    pub revision: u64,
    /// The exact strong entity tag.
    pub etag: aex_wire::types::ETag,
    /// Exact sandbox destination selected at session admission.
    pub mount_path: aex_wire::ids::FilePath,
    /// The canonical registered-file value used for materialization.
    pub value: models::RegisteredFileRead,
}

impl ResolvedInitialFile {
    /// Exact payload byte size declared by the registered content reference.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when the public size cannot fit the runtime's
    /// `u64` transfer authority.
    pub fn size_bytes(&self) -> Result<u64, AppError> {
        u64::try_from(self.value.content.size_bytes.get()).map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "registered file",
                reason: "the content size does not fit the runtime transfer authority",
            })
        })
    }
}

/// The immutable facts a private create claim must elect before side effects.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedSessionCreate {
    /// Authenticated create envelope.
    pub command: CreateSession,
    /// The final session identity.
    pub session: SessionId,
    /// The root agent identity.
    pub root_agent: AgentId,
    /// Exact provider generation identity.
    pub generation: Option<GenerationId>,
    /// Immutable runtime definition.
    pub pinned_runtime: Option<PinnedRuntime>,
    /// Dedicated BYOK credential version.
    pub provider_credential: ProviderCredentialPin,
    /// Complete resolved public configuration.
    pub resolved: ResolvedConfigAuthority,
    /// Exact effective-limit revision and execution map used by Brain.
    pub limits_revision: u64,
    /// Internal current catalog identity used by Brain. It is deliberately not
    /// part of the public resolved configuration.
    pub catalog_revision: String,
    /// Revisioned execution ceilings.
    pub agent_execution: crate::ports::AgentExecutionLimits,
    /// Revisioned per-message budget ceilings.
    pub run_budget: crate::ports::RunBudgetLimits,
    /// Account projection revision rechecked after synchronous launch work.
    pub account_revision: aex_session_domain::AccountRevision,
    /// Root-plus-subagent materialization ceiling used to derive active children.
    pub materialized_agents: u64,
    /// Canonical caller metadata.
    pub metadata: Option<SessionMetadata>,
    /// Selected immutable registry revisions in request order.
    pub initial_files: Vec<ResolvedInitialFile>,
    /// Frozen MCP transports with secret values replaced by custody names.
    pub mcp_servers: Vec<aex_brain_domain::mcp::FrozenMcpServer>,
    /// When the private claim was prepared.
    pub prepared_at: aex_wire::types::Timestamp,
}

/// Readiness evidence supplied only after all create effects finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadySessionLaunch {
    /// Exact generation the provider launched.
    pub generation: Option<GenerationId>,
    /// Provider-authoritative launch instant.
    pub launched_at: aex_wire::types::Timestamp,
    /// Registered names and revisions materialized into the guest, in order.
    pub materialized_files: Vec<(ResourceName, u64)>,
    /// Physical root-control evidence after durable `AgentStarted` sequence zero.
    pub root_started: RootStartedEvidence,
}

/// Exact physical root-control evidence used to fence public publication.
///
/// Sequence zero is a real Brain journal position. `journal_tail_hash` is what
/// distinguishes it from a fresh control whose numeric tail is also zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootStartedEvidence {
    /// Exact root agent.
    pub agent: AgentId,
    /// Exact owning session.
    pub session: SessionId,
    /// Exact elected generation.
    pub generation: Option<GenerationId>,
    /// Provider-observation instant of the durable ready transition.
    pub occurred_at: aex_wire::types::Timestamp,
    /// Physical control revision after the append.
    pub revision: AgentRevision,
    /// Physical journal tail; sequence zero is valid and expected initially.
    pub journal_tail: JournalSeq,
    /// BLAKE3 identity of the exact `AgentStarted` body at the tail.
    pub journal_tail_hash: [u8; 32],
}

/// Either a private preparation or the exact stored response of an earlier winner.
#[derive(Debug, Clone, PartialEq)]
pub enum PrepareSessionCreateOutcome {
    /// No final receipt existed; this value must be durably elected before effects.
    Prepared(Box<PreparedSessionCreate>),
    /// The same identity already committed. No volatile dependency was read.
    Replayed {
        /// The generated public resource stored by the winner.
        session: Box<models::Session>,
        /// The exact canonical response bytes stored in the receipt.
        canonical_response: Vec<u8>,
    },
}

/// Admits a session and everything born with it.
///
/// # Errors
///
/// Returns [`AppError`] for a paused account, a failed port read, an
/// unqualified provider and model pair, an absent or revoked provider
/// credential, a network policy or package ecosystem this deployment cannot
/// serve, a workspace over its ceiling, or a plan that is not submittable.
pub async fn prepare_session_create(
    context: &AppContext<'_>,
    command: &CreateSession,
) -> Result<PrepareSessionCreateOutcome, AppError> {
    let now = context.clock.now();
    if let Some(stored) = context
        .sessions
        .load_receipt(command.workspace, CREATE_SCOPE, &command.identity, now)
        .await?
    {
        return replay_session_create_receipt(&stored, command);
    }
    validate_sandbox_opt_out(command)?;
    let candidate_credential =
        aex_wire::ids::ProviderCredentialId::from_uuid7(context.ids.next_uuid_v7());
    let prepared = prepare_create(context, command, candidate_credential).await?;

    // The hidden credential and the session share one UUID payload. The
    // ciphertext writer elects that payload under the create replay identity,
    // so concurrent first attempts converge on both the same key binding and
    // the same session instead of leaving an orphan credential behind.
    let session_id: SessionId = aex_wire::ids::PrefixedId::from_uuid7(
        aex_wire::ids::PrefixedId::uuid7(&prepared.credential.credential),
    );
    if let Some(servers) = command.request.mcp_servers.as_deref()
        && !servers.is_empty()
    {
        context
            .credentials
            .bind_session_mcp_config(
                command.workspace,
                command.organization,
                session_id,
                servers,
                &command.identity,
                now,
            )
            .await?;
    }
    let mcp_servers = frozen_mcp_servers(session_id, command.request.mcp_servers.as_deref())?;
    let root_agent: AgentId = aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let sandbox_enabled = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.enabled)
        .unwrap_or(true);
    let generation = sandbox_enabled.then(|| GenerationId::from_uuid7(context.ids.next_uuid_v7()));
    let pinned = generation
        .map(|generation| {
            pinned_runtime(
                prepared.deployment,
                command,
                session_id,
                generation,
                prepared.size,
                prepared.network,
                prepared.limits_revision,
            )
        })
        .transpose()?;

    let resolved = resolved_config(
        command,
        &prepared.qualified,
        prepared.size,
        &prepared.selectors,
    )?;
    let metadata = command
        .request
        .metadata
        .as_ref()
        .map(canonical_metadata)
        .transpose()?;

    Ok(PrepareSessionCreateOutcome::Prepared(Box::new(
        PreparedSessionCreate {
            command: command.clone(),
            session: session_id,
            root_agent,
            generation,
            pinned_runtime: pinned,
            provider_credential: ProviderCredentialPin {
                credential: prepared.credential.credential,
                provider: prepared.credential.provider,
                source_generation: prepared.credential.source_generation,
                revision: prepared.credential.revision,
            },
            resolved,
            limits_revision: prepared.limits_revision,
            catalog_revision: prepared.qualified.catalog_revision.clone(),
            agent_execution: prepared.agent_execution,
            run_budget: prepared.run_budget,
            account_revision: prepared.account_revision,
            materialized_agents: prepared.materialized_agents,
            metadata,
            initial_files: prepared.initial_files,
            mcp_servers,
            prepared_at: now,
        },
    )))
}

fn validate_sandbox_opt_out(command: &CreateSession) -> Result<(), AppError> {
    let enabled = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.enabled)
        .unwrap_or(true);
    if enabled {
        return Ok(());
    }
    let has_mounts = command
        .request
        .registered
        .as_ref()
        .is_some_and(|registered| !registered.mounts.is_empty());
    let has_packages = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.packages.as_ref())
        .is_some_and(|packages| !packages.is_empty());
    let has_sandbox_mcp = command
        .request
        .mcp_servers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|server| matches!(server.transport, models::McpTransport::SandboxProcess(_)));
    if has_mounts || has_packages || has_sandbox_mcp {
        return Err(AppError::Conflict(ErrorCode::InvalidRequest));
    }
    Ok(())
}

fn frozen_mcp_servers(
    session: SessionId,
    servers: Option<&[models::McpServer]>,
) -> Result<Vec<aex_brain_domain::mcp::FrozenMcpServer>, AppError> {
    use aex_brain_domain::mcp::{FrozenMcpServer, FrozenMcpTransport, mcp_secret_name};

    servers
        .unwrap_or_default()
        .iter()
        .map(|server| {
            let transport = match &server.transport {
                models::McpTransport::RemoteHttp(remote) => FrozenMcpTransport::RemoteHttp {
                    endpoint: remote.url.as_str().to_owned(),
                    headers: remote
                        .headers
                        .as_ref()
                        .into_iter()
                        .flatten()
                        .map(|(name, _)| {
                            (
                                name.clone(),
                                mcp_secret_name(session, &server.name, "header", name),
                            )
                        })
                        .collect(),
                },
                models::McpTransport::SandboxProcess(process) => {
                    FrozenMcpTransport::SandboxProcess {
                        command: process.command.clone(),
                        args: process.args.clone().unwrap_or_default(),
                        environment: process
                            .environment
                            .as_ref()
                            .into_iter()
                            .flatten()
                            .map(|(name, _)| {
                                (
                                    name.clone(),
                                    mcp_secret_name(session, &server.name, "environment", name),
                                )
                            })
                            .collect(),
                        working_directory: process
                            .working_directory
                            .as_ref()
                            .map(ToString::to_string),
                    }
                }
            };
            Ok(FrozenMcpServer {
                name: server.name.clone(),
                transport,
            })
        })
        .collect()
}

/// Projects one strongly read create receipt without touching mutable planning
/// dependencies.
///
/// Serving adapters use this after an ambiguous final publication. Keeping the
/// check here ensures normal replay and commit recovery validate identical
/// identity, resource and canonical-response facts.
///
/// # Errors
///
/// Returns [`AppError`] if the identity conflicts with the stored intent or the
/// receipt does not contain a consistent canonical session response.
pub fn replay_session_create_receipt(
    stored: &IdempotencyReceipt,
    command: &CreateSession,
) -> Result<PrepareSessionCreateOutcome, AppError> {
    match replay(stored, &command.identity.intent()) {
        ReplayDecision::Conflict(code) => Err(AppError::Conflict(code)),
        ReplayDecision::ReturnOriginal(ReceiptOutcome::Resource {
            kind: ResourceKind::Session,
            id,
            response,
        }) => {
            let bytes =
                response
                    .inline()
                    .ok_or(AppError::Port(crate::ports::PortError::Corrupt {
                        kind: "session create receipt",
                        reason: "the stored session response is not inline",
                    }))?;
            let session: models::Session = serde_json::from_slice(bytes).map_err(|_| {
                AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "session create receipt",
                    reason: "the stored session response is malformed",
                })
            })?;
            if id.0 != session.id.to_string() {
                return Err(AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "session create receipt",
                    reason: "the stored resource id disagrees with its response",
                }));
            }
            Ok(PrepareSessionCreateOutcome::Replayed {
                session: Box::new(session),
                canonical_response: bytes.to_vec(),
            })
        }
        ReplayDecision::ReturnOriginal(_) => {
            Err(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "session create receipt",
                reason: "the stored receipt does not name a session resource",
            }))
        }
    }
}

/// Publishes a session only after exact-generation readiness is proven.
///
/// # Errors
///
/// Returns [`AppError`] if the evidence names another generation, omits or
/// reorders an elected registered-file revision, lacks `AgentStarted`, or if
/// the provider launch instant cannot express the fixed eight-hour fence.
pub fn publish_ready_session(
    prepared: &PreparedSessionCreate,
    readiness: &ReadySessionLaunch,
) -> Result<Planned<Session>, AppError> {
    let expected_files = prepared
        .initial_files
        .iter()
        .map(|file| (file.name.clone(), file.revision))
        .collect::<Vec<_>>();
    if readiness.generation != prepared.generation
        || readiness.materialized_files != expected_files
        || readiness.root_started.agent != prepared.root_agent
        || readiness.root_started.session != prepared.session
        || readiness.root_started.generation != prepared.generation
        || readiness.root_started.occurred_at < readiness.launched_at
        || readiness.root_started.revision.0 == 0
    {
        return Err(AppError::Port(crate::ports::PortError::Corrupt {
            kind: "session create readiness",
            reason: "readiness does not prove the elected generation, file revisions and root AgentStarted",
        }));
    }
    let mut lifecycle = prepared
        .generation
        .map_or_else(
            || SessionLifecycle::sandbox_disabled(readiness.launched_at),
            |generation| SessionLifecycle::launched(generation, readiness.launched_at),
        )
        .map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "session lifecycle",
                reason: "the current instant cannot express the eight-hour provider lifetime",
            })
        })?;
    if prepared.generation.is_some() {
        lifecycle.begin_suspend()?;
        lifecycle.complete_suspend(readiness.root_started.occurred_at)?;
    }
    let session = Session {
        id: prepared.session,
        workspace: prepared.command.workspace,
        organization: prepared.command.organization,
        status: SessionStatus::Idle,
        lifecycle,
        revision: SessionRevision::INITIAL,
        active_run: None,
        work_admission: WorkAdmission::Open,
        cancellation: aex_session_domain::CancellationEpoch::INITIAL,
        deletion: DeletionGuard::live(prepared.session),
        mutation_guard: None,
        root_agent: prepared.root_agent,
        generation: prepared.generation,
        pinned_runtime: prepared.pinned_runtime.clone(),
        provider_credential: prepared.provider_credential,
        lineage: aex_session_domain::Lineage::ROOT,
        resolved: prepared.resolved.clone(),
        metadata: prepared.metadata.clone(),
        created_at: prepared.prepared_at,
        updated_at: readiness.root_started.occurred_at,
    };

    let receipt = IdempotencyReceipt {
        key: ReceiptKey::of(CREATE_SCOPE, &prepared.command.identity).map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "idempotency scope",
                reason: "the create scope is a compile-time constant and must always be usable",
            })
        })?,
        identity: prepared.command.identity.clone(),
        intent: prepared.command.identity.intent(),
        outcome: ReceiptOutcome::Resource {
            kind: ResourceKind::Session,
            id: ResourceId(prepared.session.to_string()),
            // The bytes that were sent, not a recipe for re-rendering them.
            response: ResponseBody::of(&crate::projection::canonical_session_bytes(&session)?),
        },
        created_at: readiness.root_started.occurred_at,
        expires_at: None,
    };

    let plan = build_plan(
        &session,
        &readiness.root_started,
        prepared.account_revision,
        receipt,
    );
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: session,
    })
}

struct PreparedCreate<'a> {
    deployment: &'a crate::ports::DeploymentFacts,
    qualified: crate::ports::QualifiedModel,
    size: ComputeSize,
    network: aex_runtime_control::generation::NetworkPolicy,
    limits_revision: u64,
    agent_execution: crate::ports::AgentExecutionLimits,
    run_budget: crate::ports::RunBudgetLimits,
    account_revision: aex_session_domain::AccountRevision,
    materialized_agents: u64,
    selectors: Vec<RegistrySelector>,
    initial_files: Vec<ResolvedInitialFile>,
    credential: crate::ports::ProviderCredentialBinding,
}

async fn prepare_create<'a>(
    context: &AppContext<'a>,
    command: &CreateSession,
    candidate_credential: aex_wire::ids::ProviderCredentialId,
) -> Result<PreparedCreate<'a>, AppError> {
    let catalog = context
        .catalog
        .ok_or(AppError::Port(crate::ports::PortError::Unowned {
            kind: "model catalog",
            seam: "the serving process did not bind a verified model catalog",
        }))?;
    let deployment = context.deployment.ok_or(AppError::Port(
        crate::ports::PortError::Unowned {
            kind: "deployment facts",
            seam: "the serving process did not bind a validated Hands image and capability catalog",
        },
    ))?;
    let projection = context.accounts.projection(command.organization).await?;
    pause_gate(CommandClass::PausableMutation, &projection)?;

    // Pure, in-process, no `.await`: an unknown provider, an unknown model and
    // an unqualified pair are all decided before a single byte is read (D-11).
    let qualified = catalog
        .admit(command.request.provider, &command.request.model)
        .map_err(|refusal| AppError::Conflict(refusal.code()))?;

    let size = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.compute.as_ref())
        .and_then(|compute| compute.size)
        .unwrap_or(ComputeSize::DEFAULT);
    let network = network_policy(deployment, command)?;
    check_package_ecosystems(deployment, command)?;

    // One read, one revision. The values decide `limit_exceeded`; the revision
    // is pinned into the generation, so the definition names the exact policy
    // that admitted it (cluster G's recommendation, decided here because the
    // create transaction owns it).
    let bundle = context.limits.bundle(command.workspace).await?;
    // A workspace whose limits were never materialized has no resolved row, and
    // `require` says so rather than defaulting. That is the loud version of a
    // workspace that would otherwise admit into an unbounded session.
    let materialized_ceiling = bundle
        .limits
        .require(LimitId::SessionMaterializedAgents)
        .map_err(|_| AppError::Port(crate::ports::PortError::NotFound { kind: "limit" }))?;
    let initial_files_count_ceiling = bundle
        .limits
        .require(LimitId::SessionInitialFilesCount)
        .map_err(|_| AppError::Port(crate::ports::PortError::NotFound { kind: "limit" }))?;
    // The create materializes exactly one agent: the root. There is no
    // registered per-workspace session-count limit anywhere in the registry, so
    // this is the whole of `limit_exceeded` at create — which is also why A
    // D-8's "no transactional counter" holds trivially here.
    if materialized_ceiling < 1 {
        return Err(AppError::Conflict(ErrorCode::LimitExceeded));
    }
    let initial_file_ceiling = bundle
        .limits
        .require(LimitId::SessionInitialFilesBytes)
        .map_err(|_| AppError::Port(crate::ports::PortError::NotFound { kind: "limit" }))?
        .min(INITIAL_FILES_HARD_MAX_BYTES);

    if command.request.provider_api_key.is_empty() {
        return Err(AppError::Conflict(ErrorCode::InvalidRequest));
    }
    let credential = context
        .credentials
        .bind_session_api_key(
            command.workspace,
            command.organization,
            candidate_credential,
            command.request.provider,
            &command.request.provider_api_key,
            &command.identity,
            context.clock.now(),
        )
        .await?;

    let selectors = selectors_of(command);
    let initial_files = resolve_initial_files(
        context,
        command,
        &selectors,
        initial_files_count_ceiling,
        initial_file_ceiling,
    )
    .await?;

    Ok(PreparedCreate {
        deployment,
        qualified,
        size,
        network,
        limits_revision: bundle.revision,
        agent_execution: bundle.agent_execution,
        run_budget: bundle.run_budget,
        account_revision: projection.revision,
        materialized_agents: materialized_ceiling,
        selectors,
        initial_files,
        credential,
    })
}

/// Builds the exact immutable root `AgentStarted` record elected before launch.
///
/// The values come only from the revision-bound effective-limit bundle and the
/// release-qualified configuration. No Brain test default can enter a serving
/// session through this constructor.
///
/// # Errors
///
/// Returns [`AppError`] if the resolved configuration or its pinned identifiers
/// cannot be represented by Brain's immutable launch record.
pub fn initial_root_record(
    prepared: &PreparedSessionCreate,
) -> Result<aex_brain_domain::JournalRecord, AppError> {
    use aex_brain_domain::budget::{Dimension, DimensionVector};
    use aex_brain_domain::ids::{CatalogPin, ModelSlug};
    use aex_brain_domain::wire_pending::{AgentLimits, ResolvedAgentConfig, SessionCredentialPin};

    let resolved: models::ResolvedConfig =
        serde_json::from_value(prepared.resolved.document().to_value()).map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "resolved configuration",
                reason: "the elected document no longer decodes",
            })
        })?;
    let catalog_pin = CatalogPin::from_wire(&prepared.catalog_revision).map_err(|_| {
        AppError::Port(crate::ports::PortError::Corrupt {
            kind: "model catalog pin",
            reason: "the release-qualified catalog revision is malformed",
        })
    })?;
    let model = ModelSlug::new(resolved.model).map_err(|_| {
        AppError::Port(crate::ports::PortError::Corrupt {
            kind: "qualified model",
            reason: "the release-qualified model exceeds Brain's bound",
        })
    })?;
    let credential = SessionCredentialPin::new(
        prepared.provider_credential.credential,
        prepared.provider_credential.revision,
        prepared.provider_credential.source_generation,
        0,
    )
    .ok_or(AppError::Port(crate::ports::PortError::Corrupt {
        kind: "provider credential pin",
        reason: "the elected credential revision and generation must be positive",
    }))?;
    let mut budget = DimensionVector::ZERO;
    budget.set(
        Dimension::TotalChildrenCreated,
        prepared
            .run_budget
            .total_children_created
            .min(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION),
    );
    budget.set(Dimension::ProviderCalls, prepared.run_budget.provider_calls);
    budget.set(Dimension::HandsCalls, prepared.run_budget.hands_calls);
    // BYOK provider calls and the in-guest Bash tool do not charge this hosted-tool dimension.
    budget.set(Dimension::CostMicroUsd, 0);
    budget.set(
        Dimension::ActiveChildren,
        prepared
            .materialized_agents
            .saturating_sub(1)
            .min(prepared.run_budget.total_children_created)
            .min(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION),
    );
    budget.set(
        Dimension::QueuedChildren,
        prepared
            .run_budget
            .queued_children
            .min(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION),
    );
    budget.set(
        Dimension::RetainedResultBytes,
        prepared.run_budget.retained_result_bytes,
    );
    Ok(aex_brain_domain::JournalRecord::AgentStarted {
        config: Box::new(ResolvedAgentConfig {
            catalog_pin,
            provider: resolved.provider,
            credential,
            model,
            system: None,
            // Bash is release-built into Hands and has no hosted tool manifest.
            tool_manifest_digests: Vec::new(),
            mcp_servers: prepared.mcp_servers.clone(),
            hands_generation: prepared.generation,
            limits_revision: prepared.limits_revision,
            limits: AgentLimits {
                turn_deadline_ms: prepared.agent_execution.turn_deadline_ms,
                max_run_duration_ms: prepared.run_budget.max_run_duration_ms,
                max_depth: prepared
                    .agent_execution
                    .max_depth
                    .min(aex_wire::limits::MAX_SUBAGENT_DEPTH),
                max_fanout: prepared
                    .agent_execution
                    .max_fanout
                    .min(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION as u32),
            },
        }),
        parent: None,
        join: None,
        depth: 0,
        budget,
    })
}

/// The exactly-once transaction.
fn build_plan(
    session: &Session,
    root_started: &RootStartedEvidence,
    account_revision: aex_session_domain::AccountRevision,
    receipt: IdempotencyReceipt,
) -> SessionTransaction {
    // Head/receipt guards merge into their writes. The already-started root
    // control and commit-time active-account projection are two read-only
    // conditions, keeping ready publication at four constant actions.
    let conditions = vec![
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::SessionAuthority,
            partition: session.id.to_string(),
            sort: "HEAD".to_owned(),
        }),
        Condition::AccountActiveAtLeast {
            workspace: session.workspace,
            organization: session.organization,
            at_least: account_revision,
        },
        Condition::AgentRevision {
            session: session.id,
            agent: root_started.agent,
            expected: root_started.revision,
        },
        Condition::JournalTail {
            session: session.id,
            agent: root_started.agent,
            expected: root_started.journal_tail,
        },
        Condition::JournalTailHash {
            session: session.id,
            agent: root_started.agent,
            expected: root_started.journal_tail_hash,
        },
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::Idempotency,
            partition: receipt.key.scope().to_owned(),
            sort: receipt.key.key_sha256().to_owned(),
        }),
    ];

    let writes = vec![
        Write::PutSessionHead(Box::new(session.clone())),
        Write::PutSessionReceiptDirectory {
            session: session.id,
            receipt: Box::new(receipt.clone()),
        },
        Write::PutIdempotencyReceipt(Box::new(receipt)),
    ];

    SessionTransaction {
        intent: TransactionIntent::CreateSession,
        conditions,
        writes,
        // AgentStarted and materialization already committed behind the private
        // claim. Publishing the head is the only public creation fact.
        after_commit: Vec::new(),
    }
}

/// The guest network policy, refused when this plane cannot supply it.
fn network_policy(
    deployment: &crate::ports::DeploymentFacts,
    command: &CreateSession,
) -> Result<aex_runtime_control::generation::NetworkPolicy, AppError> {
    use aex_runtime_control::generation::NetworkPolicy;

    let mode = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.network.as_ref())
        .map_or(models::NetworkMode::None, |network| network.hands.mode);
    match mode {
        models::NetworkMode::None => Ok(NetworkPolicy::None),
        models::NetworkMode::PublicInternet => {
            if deployment.public_internet_egress {
                Ok(NetworkPolicy::PublicInternet)
            } else {
                // Accepting this and quietly giving the guest no egress is the
                // silent fallback the standing policy forbids outright.
                Err(AppError::Conflict(ErrorCode::InvalidNetworkPolicy))
            }
        }
    }
}

/// Refuses a package whose ecosystem no published image carries.
fn check_package_ecosystems(
    deployment: &crate::ports::DeploymentFacts,
    command: &CreateSession,
) -> Result<(), AppError> {
    let Some(packages) = command
        .request
        .sandbox
        .as_ref()
        .and_then(|sandbox| sandbox.packages.as_ref())
    else {
        return Ok(());
    };
    for package in packages {
        if !deployment.package_ecosystems.contains(&package.ecosystem) {
            return Err(AppError::Conflict(ErrorCode::UnsupportedPackageEcosystem));
        }
    }
    Ok(())
}

/// Every registry pointer the request names, in canonical order.
fn selectors_of(command: &CreateSession) -> Vec<RegistrySelector> {
    let Some(registered) = command.request.registered.as_ref() else {
        return Vec::new();
    };
    registered
        .mounts
        .iter()
        .map(|name| RegistrySelector {
            workspace: command.workspace,
            kind: RegistryKind::File,
            name: name.name.clone(),
        })
        .collect()
}

/// Proves every selected file exists at one exact registry revision and fits.
///
/// Session content is not copied, pinned, or sealed into an S3-backed filesystem root. The
/// resolved configuration retains the requested registry names and the registry/content
/// authorities continue to own their bodies independently of session lifetime.
async fn resolve_initial_files(
    context: &AppContext<'_>,
    command: &CreateSession,
    selectors: &[RegistrySelector],
    count_ceiling: u64,
    bytes_ceiling: u64,
) -> Result<Vec<ResolvedInitialFile>, AppError> {
    if u64::try_from(selectors.len()).unwrap_or(u64::MAX) > count_ceiling {
        return Err(AppError::Conflict(ErrorCode::LimitExceeded));
    }
    if selectors.is_empty() {
        return Ok(Vec::new());
    }
    let pointers = context
        .registry
        .read_many(command.workspace, selectors)
        .await?;
    if pointers.len() != selectors.len() {
        return Err(AppError::Port(crate::ports::PortError::NotFound {
            kind: "registered resource",
        }));
    }
    let mut by_name = BTreeMap::new();
    for pointer in pointers {
        if pointer.row.workspace != command.workspace || pointer.row.kind != RegistryKind::File {
            return Err(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "registered file",
                reason: "the registry reader returned a foreign selector",
            }));
        }
        let digest = pointer.value_doc.digest();
        if pointer.row.sha256 != digest
            || pointer.row.size_bytes != pointer.value_doc.size_bytes()
            || pointer.row.etag
                != aex_workspace_domain::etag_of(pointer.row.kind, pointer.row.revision, &digest)
        {
            return Err(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "registered file pointer",
                reason: "row metadata does not identify its exact value document revision",
            }));
        }
        let name = pointer.row.name.clone();
        if by_name.insert(name, pointer).is_some() {
            return Err(AppError::Port(crate::ports::PortError::Corrupt {
                kind: "registered file",
                reason: "the registry reader returned one selector twice",
            }));
        }
    }
    let mut total = 0_u64;
    let mut resolved = Vec::with_capacity(selectors.len());
    for selector in selectors {
        let pointer = by_name.remove(&selector.name).ok_or(AppError::Port(
            crate::ports::PortError::NotFound {
                kind: "registered resource",
            },
        ))?;
        let value: models::RegisteredFileRead = serde_json::from_str(pointer.value_doc.as_str())
            .map_err(|_| {
                AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "registered file",
                    reason: "the value document is not a canonical registered-file value",
                })
            })?;
        let file = ResolvedInitialFile {
            name: selector.name.clone(),
            revision: pointer.row.revision.0,
            etag: pointer.row.etag,
            mount_path: command
                .request
                .registered
                .as_ref()
                .and_then(|registered| {
                    registered
                        .mounts
                        .iter()
                        .find(|mount| mount.name == selector.name)
                })
                .map(|mount| mount.path.clone())
                .ok_or(AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "registered file",
                    reason: "the selected file has no matching mount path",
                }))?,
            value,
        };
        total = total
            .checked_add(file.size_bytes()?)
            .ok_or(AppError::Conflict(ErrorCode::LimitExceeded))?;
        if total > bytes_ceiling {
            return Err(AppError::Conflict(ErrorCode::LimitExceeded));
        }
        resolved.push(file);
    }
    if !by_name.is_empty() {
        return Err(AppError::Port(crate::ports::PortError::Corrupt {
            kind: "registered file",
            reason: "the registry reader returned an unrequested selector",
        }));
    }
    Ok(resolved)
}

/// The canonical, content-addressed configuration the session resolved.
fn resolved_config(
    command: &CreateSession,
    _qualified: &crate::ports::QualifiedModel,
    size: ComputeSize,
    _selectors: &[RegistrySelector],
) -> Result<ResolvedConfigAuthority, AppError> {
    use aex_runtime_control::shape::ShapeCapacity as _;

    let registered = command
        .request
        .registered
        .clone()
        .unwrap_or(models::SessionRegisteredSelection { mounts: Vec::new() });

    let sandbox = command.request.sandbox.as_ref();
    let sandbox_enabled = sandbox.and_then(|sandbox| sandbox.enabled).unwrap_or(true);
    let packages = sandbox
        .and_then(|sandbox| sandbox.packages.clone())
        .unwrap_or_default();
    let network_mode = sandbox
        .and_then(|sandbox| sandbox.network.as_ref())
        .map_or(models::NetworkMode::None, |network| network.hands.mode);
    let mcp_server_names = command
        .request
        .mcp_servers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|server| server.name.clone())
        .collect();

    let document = models::ResolvedConfig {
        compute: sandbox_enabled.then_some(models::ResolvedCompute {
            size,
            baseline: models::ComputeShape {
                memory_mi_b: memory_mib(size.baseline_memory_bytes()),
                vcpus: millicpu_to_vcpus(size.baseline_millicpu()),
            },
            peak: models::ComputeShape {
                memory_mi_b: memory_mib(size.peak_memory_bytes()),
                vcpus: millicpu_to_vcpus(size.peak_millicpu()),
            },
            max_disk_gi_b: gibibytes(size.disk_bytes()),
            endpoint_bandwidth_m_bps: megabytes(size.network_bytes_per_second()),
            max_concurrent_connections: size.max_connections(),
        }),
        lifecycle: models::SessionLifecyclePolicy {
            maximum_lifetime_seconds: 28_800,
            maximum_subagent_depth: u32::from(aex_wire::limits::MAX_SUBAGENT_DEPTH),
            maximum_subagents: u32::try_from(aex_wire::limits::MAX_SUBAGENTS_PER_SESSION)
                .expect("the launch subagent ceiling fits u32"),
        },
        mcp_server_names,
        model: command.request.model.clone(),
        network: sandbox_enabled.then_some(models::ResolvedNetwork {
            hands: models::HandsNetworkRequest { mode: network_mode },
        }),
        packages,
        provider: command.request.provider,
        registered,
        sandbox_enabled,
    };
    let canonical = CanonicalJson::parse(&to_jcs_string(&document)?)?;
    ResolvedConfigAuthority::new(
        canonical,
        command.request.provider,
        command.request.model.clone(),
    )
    .map_err(|_| {
        AppError::Port(crate::ports::PortError::Corrupt {
            kind: "resolved configuration",
            reason: "the document this create just built does not match its own projections",
        })
    })
}

/// The immutable generation definition the create decides and never revisits.
#[allow(
    clippy::too_many_arguments,
    reason = "every argument is a distinct decided fact and grouping them into a struct would \
              only move the same list one line up"
)]
fn pinned_runtime(
    deployment: &crate::ports::DeploymentFacts,
    command: &CreateSession,
    session: SessionId,
    generation: GenerationId,
    size: ComputeSize,
    network: aex_runtime_control::generation::NetworkPolicy,
    limits_revision: u64,
) -> Result<PinnedRuntime, AppError> {
    let image = deployment
        .images
        // No capability is required today: the browser layer has no request
        // field, so asking for it here would be inventing a selector the
        // contract does not have.
        .select(size, &[])
        .map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "image catalog",
                reason: "the boot-validated catalog cannot serve a size the contract admits",
            })
        })?;
    let definition = aex_runtime_control::generation::HandsGeneration {
        generation,
        session,
        workspace: command.workspace,
        organization: command.organization,
        size,
        image,
        network,
        protocol_version: aex_internal_contracts::SchemaVersion::V1,
        limits_revision: aex_runtime_control::generation::LimitsRevision(limits_revision),
        root: aex_runtime_control::generation::guest_root(),
    };
    PinnedRuntime::new(session, command.workspace, command.organization, definition).map_err(|_| {
        AppError::Port(crate::ports::PortError::Corrupt {
            kind: "pinned generation",
            reason: "the definition this create just built does not name its own session",
        })
    })
}

fn canonical_metadata(
    metadata: &BTreeMap<String, aex_wire::types::MetadataValue>,
) -> Result<SessionMetadata, AppError> {
    let canonical = CanonicalJson::parse(&to_jcs_string(metadata)?)?;
    SessionMetadata::new(canonical).map_err(|_| AppError::Conflict(ErrorCode::InvalidRequest))
}

fn memory_mib(bytes: u64) -> u32 {
    u32::try_from(bytes / (1024 * 1024)).unwrap_or(u32::MAX)
}

fn gibibytes(bytes: u64) -> u32 {
    u32::try_from(bytes / (1024 * 1024 * 1024)).unwrap_or(u32::MAX)
}

fn megabytes(bytes_per_second: u64) -> u32 {
    u32::try_from(bytes_per_second / 1_000_000).unwrap_or(u32::MAX)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a millicpu figure is at most six digits, well inside `f64`'s exact integer range"
)]
fn millicpu_to_vcpus(millicpu: u32) -> f64 {
    f64::from(millicpu) / 1000.0
}
