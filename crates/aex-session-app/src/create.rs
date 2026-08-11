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
    AgentControl, AgentKind, AgentStatus, CommandClass, DeletionGuard, IdempotencyIdentity,
    IdempotencyReceipt, OpenEffectSet, PinnedRuntime, ProviderCredentialPin, ReceiptKey,
    ReceiptOutcome, ReplayDecision, ResolvedConfigAuthority, ResourceId, ResourceKind,
    ResponseBody, Session, SessionLifecycle, SessionMetadata, SessionRevision, SessionStatus,
    WorkAdmission, pause_gate, replay,
};
use aex_wire::canonical::{CanonicalJson, to_jcs_string};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{AgentId, GenerationId, ResourceName, SessionId, WorkspaceId};
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
    /// The canonical registered-file value used for materialization.
    pub value: models::RegisteredFileRead,
}

impl ResolvedInitialFile {
    /// Exact payload byte size declared by the registered content reference.
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
    pub generation: GenerationId,
    /// Immutable runtime definition.
    pub pinned_runtime: PinnedRuntime,
    /// Dedicated BYOK credential version.
    pub provider_credential: ProviderCredentialPin,
    /// Complete resolved public configuration.
    pub resolved: ResolvedConfigAuthority,
    /// Canonical caller metadata.
    pub metadata: Option<SessionMetadata>,
    /// Selected immutable registry revisions in request order.
    pub initial_files: Vec<ResolvedInitialFile>,
    /// When the private claim was prepared.
    pub prepared_at: aex_wire::types::Timestamp,
}

/// Readiness evidence supplied only after all create effects finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadySessionLaunch {
    /// Exact generation the provider launched.
    pub generation: GenerationId,
    /// Provider-authoritative launch instant.
    pub launched_at: aex_wire::types::Timestamp,
    /// Registered names and revisions materialized into the guest, in order.
    pub materialized_files: Vec<(ResourceName, u64)>,
    /// Root control after its durable `AgentStarted` journal append.
    pub root_agent: AgentControl,
}

/// Either a private preparation or the exact stored response of an earlier winner.
#[derive(Debug, Clone, PartialEq)]
pub enum PrepareSessionCreateOutcome {
    /// No final receipt existed; this value must be durably elected before effects.
    Prepared(Box<PreparedSessionCreate>),
    /// The same identity already committed. No volatile dependency was read.
    Replayed {
        /// The generated public resource stored by the winner.
        session: models::Session,
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
        .load_receipt(
            command.workspace,
            CREATE_RECEIPT_SCOPE,
            &command.identity,
            now,
        )
        .await?
    {
        return match replay(&stored, &command.identity.intent()) {
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
                    session,
                    canonical_response: bytes.to_vec(),
                })
            }
            ReplayDecision::ReturnOriginal(_) => {
                Err(AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "session create receipt",
                    reason: "the stored receipt does not name a session resource",
                }))
            }
        };
    }
    let prepared = prepare_create(context, command).await?;

    let session_id: SessionId = aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let root_agent: AgentId = aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let generation: GenerationId =
        aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let pinned = pinned_runtime(
        prepared.deployment,
        command,
        session_id,
        generation,
        prepared.size,
        prepared.network,
        prepared.limits_revision,
    )?;

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
            metadata,
            initial_files: prepared.initial_files,
            prepared_at: now,
        },
    )))
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
        || readiness.root_agent.id != prepared.root_agent
        || readiness.root_agent.session != prepared.session
        || readiness.root_agent.generation != Some(prepared.generation)
        || readiness.root_agent.journal_tail == aex_session_domain::JournalSeq::INITIAL
        || readiness.root_agent.last_entry.is_none()
    {
        return Err(AppError::Port(crate::ports::PortError::Corrupt {
            kind: "session create readiness",
            reason: "readiness does not prove the elected generation, file revisions and root AgentStarted",
        }));
    }
    let session = Session {
        id: prepared.session,
        workspace: prepared.command.workspace,
        organization: prepared.command.organization,
        status: SessionStatus::Idle,
        lifecycle: SessionLifecycle::launched(prepared.generation, readiness.launched_at).map_err(
            |_| {
                AppError::Port(crate::ports::PortError::Corrupt {
                    kind: "session lifecycle",
                    reason: "the current instant cannot express the eight-hour provider lifetime",
                })
            },
        )?,
        revision: SessionRevision::INITIAL,
        active_run: None,
        work_admission: WorkAdmission::Open,
        cancellation: aex_session_domain::CancellationEpoch::INITIAL,
        deletion: DeletionGuard::live(prepared.session),
        mutation_guard: None,
        root_agent: prepared.root_agent,
        generation: Some(prepared.generation),
        pinned_runtime: prepared.pinned_runtime.clone(),
        provider_credential: prepared.provider_credential,
        lineage: aex_session_domain::Lineage::ROOT,
        resolved: prepared.resolved.clone(),
        metadata: prepared.metadata.clone(),
        created_at: prepared.prepared_at,
        updated_at: readiness.launched_at,
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
        created_at: readiness.launched_at,
        expires_at: None,
    };

    let plan = build_plan(&session, &readiness.root_agent, receipt);
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: session,
    })
}

/// Builds the root control row the private create claim owns before activation.
///
/// Brain appends `AgentStarted` to this exact control/journal authority. Final
/// publication conditions on the resulting revision and tail and never
/// overwrites them with an empty root.
#[must_use]
pub fn initial_root_agent(prepared: &PreparedSessionCreate) -> AgentControl {
    AgentControl {
        id: prepared.root_agent,
        session: prepared.session,
        kind: AgentKind::Root,
        parent: None,
        depth: 0,
        status: AgentStatus::Idle,
        revision: aex_session_domain::AgentRevision::INITIAL,
        journal_tail: aex_session_domain::JournalSeq::INITIAL,
        last_entry: None,
        claim: None,
        join: None,
        budget: None,
        open_effects: OpenEffectSet::default(),
        pending_approval: None,
        queue_reason: None,
        generation: Some(prepared.generation),
        terminal: None,
        created_at: prepared.prepared_at,
    }
}

struct PreparedCreate<'a> {
    deployment: &'a crate::ports::DeploymentFacts,
    qualified: crate::ports::QualifiedModel,
    size: ComputeSize,
    network: aex_runtime_control::generation::NetworkPolicy,
    limits_revision: u64,
    selectors: Vec<RegistrySelector>,
    initial_files: Vec<ResolvedInitialFile>,
    credential: crate::ports::ProviderCredentialBinding,
}

async fn prepare_create<'a>(
    context: &AppContext<'a>,
    command: &CreateSession,
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
        .compute
        .as_ref()
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

    let credential = context
        .credentials
        .read_provider_credential(command.workspace, command.request.provider_credential_id)
        .await?
        .ok_or(AppError::Conflict(ErrorCode::ProviderCredentialNotFound))?;
    if credential.provider != command.request.provider {
        // A binding for another provider is not this workspace's answer to
        // "which key signs for this provider", and pretending it is would send
        // one vendor's key to another.
        return Err(AppError::Conflict(ErrorCode::ProviderCredentialNotFound));
    }
    if credential.state == crate::ports::CredentialState::Revoked {
        return Err(AppError::Conflict(ErrorCode::ProviderCredentialRevoked));
    }

    let selectors = selectors_of(command);
    let initial_files =
        resolve_initial_files(context, command, &selectors, initial_file_ceiling).await?;

    Ok(PreparedCreate {
        deployment,
        qualified,
        size,
        network,
        limits_revision: bundle.revision,
        selectors,
        initial_files,
        credential,
    })
}

/// The exactly-once transaction.
fn build_plan(
    session: &Session,
    root_agent: &AgentControl,
    receipt: IdempotencyReceipt,
) -> SessionTransaction {
    // Every guard targets an item this plan also writes, so each merges into
    // that write's own condition expression and the plan's action count is the
    // number of writes. There is not one read-only `ConditionCheck` (A D-7).
    let conditions = vec![
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::SessionAuthority,
            partition: session.id.to_string(),
            sort: "HEAD".to_owned(),
        }),
        Condition::AgentRevision {
            session: session.id,
            agent: root_agent.id,
            expected: root_agent.revision,
        },
        Condition::JournalTail {
            session: session.id,
            agent: root_agent.id,
            expected: root_agent.journal_tail,
        },
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::Idempotency,
            partition: receipt.key.scope().to_owned(),
            sort: receipt.key.key_sha256().to_owned(),
        }),
    ];

    let writes = vec![
        Write::PutSessionHead(Box::new(session.clone())),
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
        .network
        .as_ref()
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
    let Some(packages) = command.request.packages.as_ref() else {
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
        .files
        .iter()
        .flatten()
        .map(|name| RegistrySelector {
            workspace: command.workspace,
            kind: RegistryKind::File,
            name: name.clone(),
        })
        .collect()
}

/// Proves every registered name in the request exists in this workspace.
///
/// Session content is not copied, pinned, or sealed into an S3-backed filesystem root. The
/// resolved configuration retains the requested registry names and the registry/content
/// authorities continue to own their bodies independently of session lifetime.
async fn resolve_initial_files(
    context: &AppContext<'_>,
    command: &CreateSession,
    selectors: &[RegistrySelector],
    ceiling: u64,
) -> Result<Vec<ResolvedInitialFile>, AppError> {
    if selectors.is_empty() {
        return Ok(Vec::new());
    }
    let pointers = context
        .registry
        .read_many(command.workspace, selectors)
        .await?;
    if pointers.len() != selectors.len() {
        // A name the workspace does not have is not a session that mounts
        // fewer things than it asked for.
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
            value,
        };
        total = total
            .checked_add(file.size_bytes()?)
            .ok_or(AppError::Conflict(ErrorCode::LimitExceeded))?;
        if total > ceiling {
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
    qualified: &crate::ports::QualifiedModel,
    size: ComputeSize,
    selectors: &[RegistrySelector],
) -> Result<ResolvedConfigAuthority, AppError> {
    use aex_runtime_control::shape::ShapeCapacity as _;

    let registered = models::SessionRegisteredSelection {
        files: (!selectors.is_empty()).then(|| {
            selectors
                .iter()
                .map(|selector| selector.name.clone())
                .collect()
        }),
    };

    let document = models::ResolvedConfig {
        catalog_revision: qualified.catalog_revision.clone(),
        compute: models::ResolvedCompute {
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
        },
        lifecycle: models::SessionLifecyclePolicy {
            idle_suspend_after_seconds: 180,
            maximum_lifetime_seconds: 28_800,
            resume_on_live_file_access: true,
            resume_on_message: true,
        },
        model: command.request.model.clone(),
        network: models::ResolvedNetwork {
            hands: models::HandsNetworkRequest {
                mode: command
                    .request
                    .network
                    .as_ref()
                    .map_or(models::NetworkMode::None, |network| network.hands.mode),
            },
        },
        packages: command.request.packages.clone().unwrap_or_default(),
        provider: command.request.provider,
        provider_credential_id: command.request.provider_credential_id,
        registered,
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
