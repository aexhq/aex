//! Session admission.
//!
//! One transaction, **constant** in request size: five items always plus one
//! conditional, whatever the request selected (A D-1, A D-5). The 1088
//! registered names, 64 secrets and 64 packages a maximal request may carry all
//! collapse into values that ride existing items — the selection into one sealed
//! content root, the secrets into one custody record, the packages into the
//! resolved-configuration document on the head.
//!
//! # What this use case deliberately does not do
//!
//! **It does not look up the receipt.** The receipt's conditional
//! `attribute_not_exists` put *is* the concurrency election (A D-6), and
//! `TransactWriteItems` is all-or-nothing, so a caller that loses the receipt
//! writes nothing at all. A pre-read would be a second, weaker election that
//! costs a round trip on every create and still could not decide the race. The
//! replay is resolved from the refused commit, where the answer is authoritative.
//!
//! **It does not write the Hands generation rows.** It mints the
//! [`GenerationId`] and pins the whole immutable definition on the head; the
//! `runtime-activity` generation head and `CURRENT` pointer are derived from
//! that pin, idempotently, by the first path that needs them (A D-2). No caller
//! can observe their absence at create, because H-LAZY guarantees no provider
//! call has happened and `liveGenerationId` is optional.
//!
//! **It carries no read-only condition check and no session counter** (A D-7,
//! A D-8). Every pre-transaction read is eventually consistent and every stale
//! outcome is either self-correcting or refused loudly downstream.

use std::collections::{BTreeMap, BTreeSet};

use aex_content_domain::{ContentRoot, OwnerEdge, Pin, PinSubject, RegistryKind, RootKind};
use aex_secret_domain::{OwnerKeyEdgeId, SecretName, admit_custody};
use aex_session_domain::{
    AgentControl, AgentKind, AgentStatus, CommandClass, DeletionGuard, IdempotencyIdentity,
    IdempotencyReceipt, OpenEffectSet, PinnedRuntime, ReceiptKey, ReceiptOutcome,
    ResolvedConfigAuthority, ResourceId, ResourceKind, ResponseBody, Session, SessionMetadata,
    SessionRevision, SessionStatus, WorkAdmission, pause_gate,
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
use crate::ports::{AppContext, SealedRegistryEntry};

/// The idempotency scope every create receipt is filed under.
///
/// The one subjectless base: a create names no resource yet, so the caller's
/// key is the whole identity.
pub const CREATE_SCOPE: &str = "session.create";

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

/// Admits a session and everything born with it.
///
/// # Errors
///
/// Returns [`AppError`] for a paused account, a failed port read, an
/// unqualified provider and model pair, an absent or revoked provider
/// credential, a network policy or package ecosystem this deployment cannot
/// serve, a workspace over its ceiling, or a plan that is not submittable.
pub async fn create_session(
    context: &AppContext<'_>,
    command: &CreateSession,
) -> Result<Planned<Session>, AppError> {
    let prepared = prepare_create(context, command).await?;

    let session_id: SessionId = aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let root_agent: AgentId = aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let generation: GenerationId =
        aex_wire::ids::PrefixedId::from_uuid7(context.ids.next_uuid_v7());
    let now = context.clock.now();

    let pinned = pinned_runtime(
        prepared.deployment,
        command,
        session_id,
        generation,
        prepared.size,
        prepared.network,
        prepared.limits_revision,
        &prepared.initial_root,
    )?;

    let custody = if command
        .request
        .credentials
        .as_ref()
        .is_some_and(|credentials| !credentials.secrets.is_empty())
    {
        Some(prepare_custody(context, command, session_id, now).await?)
    } else {
        None
    };

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

    let session = Session {
        id: session_id,
        workspace: command.workspace,
        organization: command.organization,
        status: SessionStatus::Idle,
        revision: SessionRevision::INITIAL,
        active_run: None,
        work_admission: WorkAdmission::Open,
        cancellation: aex_session_domain::CancellationEpoch::INITIAL,
        deletion: DeletionGuard::live(session_id),
        mutation_guard: None,
        root_agent,
        // The generation that is *live*. Nothing is running: H-LAZY means the
        // create makes no provider call at all.
        generation: None,
        pinned_runtime: pinned,
        initial_root: prepared.initial_root,
        // A new session has persisted nothing. Its durable root is its initial
        // one, which is what a first persist advances from.
        persisted_root: prepared.initial_root,
        persist_revision: aex_session_domain::PersistRevision::INITIAL,
        last_persisted_at: None,
        custody_revision: custody
            .as_ref()
            .map_or(aex_secret_domain::CustodyRevision::FIRST, |custody| {
                custody.revision
            }),
        lineage: aex_session_domain::Lineage::ROOT,
        resolved,
        metadata,
        created_at: now,
        updated_at: now,
    };

    let receipt = IdempotencyReceipt {
        key: ReceiptKey::of(CREATE_SCOPE, &command.identity).map_err(|_| {
            AppError::Port(crate::ports::PortError::Corrupt {
                kind: "idempotency scope",
                reason: "the create scope is a compile-time constant and must always be usable",
            })
        })?,
        identity: command.identity.clone(),
        intent: command.identity.intent(),
        outcome: ReceiptOutcome::Resource {
            kind: ResourceKind::Session,
            id: ResourceId(session_id.to_string()),
            // The bytes that were sent, not a recipe for re-rendering them.
            response: ResponseBody::of(&crate::projection::canonical_session_bytes(&session)?),
        },
        created_at: now,
        expires_at: None,
    };

    let plan = build_plan(&session, root_agent_record(&session, now), receipt, custody);
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
    selectors: Vec<RegistrySelector>,
    initial_root: ContentRoot,
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

    let credential = context
        .secrets
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
    let initial_root = seal(context, command, &selectors).await?;

    Ok(PreparedCreate {
        deployment,
        qualified,
        size,
        network,
        limits_revision: bundle.revision,
        selectors,
        initial_root,
    })
}

/// The exactly-once transaction.
fn build_plan(
    session: &Session,
    root_agent: AgentControl,
    receipt: IdempotencyReceipt,
    custody: Option<aex_secret_domain::SessionCustody>,
) -> SessionTransaction {
    // Every guard targets an item this plan also writes, so each merges into
    // that write's own condition expression and the plan's action count is the
    // number of writes. There is not one read-only `ConditionCheck` (A D-7).
    let mut conditions = vec![
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::SessionAuthority,
            partition: session.id.to_string(),
            sort: "HEAD".to_owned(),
        }),
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::SessionAuthority,
            partition: format!("{}#{}", session.id, session.root_agent),
            sort: "CONTROL".to_owned(),
        }),
        Condition::ItemAbsent(ItemKey {
            family: TableFamily::Idempotency,
            partition: receipt.key.scope().to_owned(),
            sort: receipt.key.key_sha256().to_owned(),
        }),
    ];

    let mut writes = vec![
        Write::PutSessionHead(Box::new(session.clone())),
        Write::PutAgentControl(Box::new(root_agent)),
        Write::PutIdempotencyReceipt(Box::new(receipt)),
    ];

    // A pin on an empty root retains nothing, so an empty seal writes neither
    // content item and a create with no selection and no secrets is three items
    // in one table.
    if session.initial_root.entries > 0 {
        let pin = Pin::Root {
            session: session.id,
            kind: RootKind::Initial,
            root: session.initial_root,
        };
        writes.push(Write::PutPin(Box::new(pin.clone())));
        writes.push(Write::PutOwnerEdge(Box::new(OwnerEdge {
            workspace: session.workspace,
            subject: PinSubject::Session(session.id),
            pin,
        })));
    }

    if let Some(custody) = custody {
        conditions.push(Condition::ItemAbsent(ItemKey {
            family: TableFamily::SecretCustody,
            partition: session.id.to_string(),
            sort: "CUSTODY".to_owned(),
        }));
        writes.push(Write::PutCustody(Box::new(custody)));
    }

    SessionTransaction {
        intent: TransactionIntent::CreateSession,
        conditions,
        writes,
        // No `session.created` event and no wake: nothing is runnable yet, and
        // the head insert on `session-authority` already *is* the creation fact
        // (A D-3).
        after_commit: Vec::new(),
    }
}

/// The session's root agent, at rest, bound to the generation the create pinned.
fn root_agent_record(session: &Session, now: aex_wire::types::Timestamp) -> AgentControl {
    AgentControl {
        id: session.root_agent,
        session: session.id,
        kind: AgentKind::Root,
        parent: None,
        depth: 0,
        // `Idle` is the root at rest and has no public child projection. A root
        // is never born `Queued`: nothing has asked it to do anything.
        status: AgentStatus::Idle,
        revision: aex_session_domain::AgentRevision::INITIAL,
        journal_tail: aex_session_domain::JournalSeq::INITIAL,
        last_entry: None,
        claim: None,
        join: None,
        // A root at rest has no run, so it has no run-local spend ceiling yet.
        budget: None,
        open_effects: OpenEffectSet::default(),
        pending_approval: None,
        queue_reason: None,
        // The generation this agent will run in, decided with the head. Not
        // `None`: the physical control row's generation is non-optional, and a
        // root that names no generation could not be launched without inventing
        // one.
        generation: Some(session.pinned_runtime.generation()),
        terminal: None,
        created_at: now,
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
    let mut selectors = Vec::new();
    let mut push = |kind: RegistryKind, names: Option<&Vec<ResourceName>>| {
        for name in names.into_iter().flatten() {
            selectors.push(RegistrySelector {
                workspace: command.workspace,
                kind,
                name: name.clone(),
            });
        }
    };
    push(RegistryKind::File, registered.files.as_ref());
    push(RegistryKind::Skill, registered.skills.as_ref());
    push(RegistryKind::Tool, registered.tools.as_ref());
    push(RegistryKind::Instruction, registered.instructions.as_ref());
    push(RegistryKind::McpServer, registered.mcp_servers.as_ref());
    selectors
}

/// Seals the selected registry resolution into the session's initial root.
///
/// The seal is a point-in-time snapshot by definition, so the transaction
/// carries **no** `Condition::RegistryEtag` (A D-4): a concurrent registry
/// mutation between this read and the commit does not invalidate the session,
/// it means the session mounted the earlier revision, which is what "sealed"
/// means. Conditioning on up to 1088 pointers would breach the 100-action
/// ceiling to enforce a property the seal does not want.
async fn seal(
    context: &AppContext<'_>,
    command: &CreateSession,
    selectors: &[RegistrySelector],
) -> Result<ContentRoot, AppError> {
    if selectors.is_empty() {
        return Ok(ContentRoot {
            digest: [0; 32],
            entries: 0,
            logical_bytes: 0,
        });
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
    let entries: Vec<SealedRegistryEntry> = pointers
        .iter()
        .map(|pointer| SealedRegistryEntry {
            kind: pointer.row.kind,
            name: pointer.row.name.clone(),
            revision: pointer.row.revision,
            digest: pointer.row.sha256,
        })
        .collect();
    Ok(context
        .content_writer
        .seal_registry_manifest(command.workspace, &entries)
        .await?)
}

/// The session's first credential custody, when the request named secrets.
async fn prepare_custody(
    context: &AppContext<'_>,
    command: &CreateSession,
    session: SessionId,
    now: aex_wire::types::Timestamp,
) -> Result<aex_secret_domain::SessionCustody, AppError> {
    let names: Vec<SecretName> = command
        .request
        .credentials
        .as_ref()
        .map(|credentials| {
            credentials
                .secrets
                .iter()
                .map(|secret| secret.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let unique: BTreeSet<&SecretName> = names.iter().collect();
    if unique.len() != names.len() {
        return Err(AppError::Custody(
            aex_secret_domain::CustodyRejection::DuplicateName(
                names
                    .iter()
                    .find(|name| names.iter().filter(|other| other == name).count() > 1)
                    .cloned()
                    .unwrap_or_else(|| names[0].clone()),
            ),
        ));
    }
    let selected = context
        .secrets
        .read_secrets(command.workspace, &names)
        .await?;
    if names
        .iter()
        .any(|name| !selected.iter().any(|secret| secret.name == *name))
    {
        return Err(crate::ports::PortError::NotFound { kind: "secret" }.into());
    }
    let edge = OwnerKeyEdgeId(context.ids.next_uuid_v7());
    Ok(admit_custody(
        session,
        command.workspace,
        None,
        &selected,
        edge,
        now,
    )?)
}

/// The canonical, content-addressed configuration the session resolved.
fn resolved_config(
    command: &CreateSession,
    qualified: &crate::ports::QualifiedModel,
    size: ComputeSize,
    selectors: &[RegistrySelector],
) -> Result<ResolvedConfigAuthority, AppError> {
    use aex_runtime_control::shape::ShapeCapacity as _;

    let mut registered = models::SessionRegisteredSelection {
        files: None,
        instructions: None,
        mcp_servers: None,
        skills: None,
        tools: None,
    };
    for selector in selectors {
        let bucket = match selector.kind {
            RegistryKind::File => &mut registered.files,
            RegistryKind::Skill => &mut registered.skills,
            RegistryKind::Tool => &mut registered.tools,
            RegistryKind::Instruction => &mut registered.instructions,
            RegistryKind::McpServer => &mut registered.mcp_servers,
        };
        bucket
            .get_or_insert_with(Vec::new)
            .push(selector.name.clone());
    }

    let document = models::ResolvedConfig {
        approval_policy: command.request.approval_policy.clone().unwrap_or(
            models::ApprovalPolicy::AllowAll(models::ApprovalPolicyAllowAll {}),
        ),
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
    _root: &ContentRoot,
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
