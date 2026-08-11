//! The application context this deployable can honestly supply.
//!
//! `aex_session_app::AppContext` names its read authorities explicitly. This
//! deployable binds the existing named-registry, revision-fenced workspace
//! limit and dedicated provider-credential readers for create/message
//! admission; only the legacy `LiveWorkspaceReader` projection remains unused.
//!
//! Two ports left this file rather than gaining a local implementation, and
//! both for the same reason: the authority already existed elsewhere.
//! `aex_runtime_activity_dynamodb::RuntimeContinuity` reads the rows
//! `aex-runtime-control` owns the idle predicate over, so there is no second
//! reading of "is this session idle";
//! `aex_secret_custody_dynamodb::SessionCustodyReads` holds both rows a custody
//! read has to join, so the sealed row keeps exactly one reader.
//!
//! [`UnownedPorts`] is **not** a fake adapter. Its methods return
//! [`PortError::Unowned`] naming the seam that owes the implementation. Writing
//! thin local versions instead is the pattern the whole regional stream refused
//! (`references/rewrite/regional-services.md:109-110`): a locally computed
//! `TrueIdle`, for instance, would mean two authorities answering "is this
//! session idle" differently. A refusal is loud, typed and unmountable; an
//! invention is silent and wrong.
//!
//! Mounted routes never reach that refusal; live files use the narrower
//! exact-generation Hands transport instead.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_registry_dynamodb::store::RegistryStore;
use aex_session_app::ports::{
    AgentExecutionLimits, IdFactory, LimitsBundle, LimitsReader, LiveEntry, LiveListQuery,
    LiveListing, LiveWorkspaceReader, PortError, RegistryReader, RunBudgetLimits,
};
use aex_session_domain::EffectiveLimits;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::WorkspaceProjection;
use aex_wire::ids::{GenerationId, SessionId, UploadId, Uuid7, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models::{EffectiveWorkspaceLimit, LimitValue};
use aex_workspace_domain::{RegistryPointer, RegistrySelector, Upload};
use futures::{StreamExt as _, TryStreamExt as _};

/// A time-ordered identifier source.
///
/// Real, not scripted: `uuid` already mints v7 and the workspace's `Uuid7`
/// checks the version and variant nibbles on construction, so an identifier
/// that is not time-ordered cannot escape this function.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestIds;

impl IdFactory for RequestIds {
    fn next_uuid_v7(&self) -> Uuid7 {
        Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
            .unwrap_or_else(|_| unreachable!("`Uuid::now_v7` always sets the v7 version nibble"))
    }
}

/// The retired broad live-workspace projection.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnownedPorts;

/// Production named-registry reads used during session admission.
#[derive(Clone)]
pub struct RegistryReads {
    store: Arc<dyn RegistryStore>,
}

impl RegistryReads {
    /// Binds the existing regional registry authority without copying its codec.
    #[must_use]
    pub fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
    }
}

impl std::fmt::Debug for RegistryReads {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistryReads")
            .finish_non_exhaustive()
    }
}

/// Revision-fenced production limit reads used during session admission.
#[derive(Clone)]
pub struct LimitReads {
    projection: Arc<dyn WorkspaceProjection>,
}

impl LimitReads {
    /// Binds the existing read-only workspace projection.
    #[must_use]
    pub fn new(projection: Arc<dyn WorkspaceProjection>) -> Self {
        Self { projection }
    }

    async fn complete_bundle(&self, workspace: WorkspaceId) -> Result<LimitsBundle, PortError> {
        // The head is strongly consistent. The payload is one internally
        // complete item and may lag by one replica horizon; a mismatch is a
        // transient refusal, never a mixed revision presented as authoritative.
        let head = self
            .projection
            .read_limit_bundle_head(workspace)
            .await
            .map_err(|error| port_error(&error, "effective limits"))?;
        let payload = self
            .projection
            .read_limit_bundle(workspace)
            .await
            .map_err(|error| port_error(&error, "effective limits"))?;
        if head.workspace != workspace
            || payload.workspace != workspace
            || head.revision != payload.revision
        {
            return Err(PortError::Unavailable {
                kind: "effective limits",
            });
        }
        project_limit_bundle(payload.revision, &payload.limits)
    }
}

impl std::fmt::Debug for LimitReads {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LimitReads").finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl RegistryReader for RegistryReads {
    async fn read_many(
        &self,
        workspace: WorkspaceId,
        selectors: &[RegistrySelector],
    ) -> Result<Vec<RegistryPointer>, PortError> {
        futures::stream::iter(selectors.iter().cloned())
            .map(|selector| {
                let store = Arc::clone(&self.store);
                async move {
                    store
                        .load_pointer(workspace, selector.kind, selector.name.as_str())
                        .await
                        .map_err(|error| port_error(&error, "registry pointer"))
                }
            })
            .buffered(16)
            .try_filter_map(|pointer| futures::future::ready(Ok(pointer)))
            .try_collect()
            .await
    }

    async fn read_upload(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Upload, PortError> {
        self.store
            .load_upload(workspace, upload)
            .await
            .map_err(|error| port_error(&error, "upload"))?
            .ok_or(PortError::NotFound { kind: "upload" })
    }
}

#[async_trait::async_trait]
impl LimitsReader for LimitReads {
    async fn effective(
        &self,
        workspace: WorkspaceId,
        ids: &[LimitId],
    ) -> Result<EffectiveLimits, PortError> {
        let bundle = self.complete_bundle(workspace).await?;
        let mut selected = EffectiveLimits::new();
        for id in ids {
            if let Ok(value) = bundle.limits.require(*id) {
                selected.insert(*id, value);
            }
        }
        Ok(selected)
    }

    async fn bundle(&self, workspace: WorkspaceId) -> Result<LimitsBundle, PortError> {
        self.complete_bundle(workspace).await
    }
}

fn project_limit_bundle(
    revision: u64,
    rows: &[EffectiveWorkspaceLimit],
) -> Result<LimitsBundle, PortError> {
    if rows.len() != LimitId::ALL.len() {
        return Err(corrupt_limits("the complete limit bundle is short"));
    }
    let mut scalars = EffectiveLimits::new();
    let mut maps = BTreeMap::new();
    for (expected, row) in LimitId::ALL.iter().copied().zip(rows) {
        if row.id != expected || row.revision != revision {
            return Err(corrupt_limits(
                "the complete limit bundle has a foreign identity or revision",
            ));
        }
        match &row.effective_value {
            LimitValue::Scalar(value) => {
                scalars.insert(expected, exact_u64(value.value.get())?);
            }
            LimitValue::Map(value) => {
                maps.insert(expected, &value.values);
            }
        }
    }
    let execution = maps
        .remove(&LimitId::SessionAgentExecution)
        .ok_or_else(|| corrupt_limits("session.agent_execution is not a map"))?;
    let run = maps
        .remove(&LimitId::SessionRunBudget)
        .ok_or_else(|| corrupt_limits("session.run_budget is not a map"))?;
    Ok(LimitsBundle {
        revision,
        limits: scalars,
        agent_execution: AgentExecutionLimits {
            max_turns: exact_u32(dimension(execution, "max_turns")?)?,
            max_steps_per_turn: exact_u32(dimension(execution, "max_steps_per_turn")?)?,
            turn_deadline_ms: exact_u32(dimension(execution, "turn_deadline_ms")?)?,
            max_depth: exact_u16(dimension(execution, "max_depth")?)?,
            max_fanout: exact_u32(dimension(execution, "max_fanout")?)?,
        },
        run_budget: RunBudgetLimits {
            max_run_duration_ms: dimension(run, "max_run_duration_ms")?,
            total_children_created: dimension(run, "total_children_created")?,
            provider_calls: dimension(run, "provider_calls")?,
            hands_calls: dimension(run, "hands_calls")?,
            queued_children: dimension(run, "queued_children")?,
            retained_result_bytes: dimension(run, "retained_result_bytes")?,
        },
    })
}

fn dimension(
    values: &BTreeMap<String, aex_wire::types::DecimalU128>,
    name: &'static str,
) -> Result<u64, PortError> {
    exact_u64(
        values
            .get(name)
            .ok_or_else(|| corrupt_limits("a revisioned limit dimension is missing"))?
            .get(),
    )
}

fn exact_u64(value: u128) -> Result<u64, PortError> {
    u64::try_from(value).map_err(|_| corrupt_limits("a limit does not fit u64"))
}

fn exact_u32(value: u64) -> Result<u32, PortError> {
    u32::try_from(value).map_err(|_| corrupt_limits("a limit does not fit u32"))
}

fn exact_u16(value: u64) -> Result<u16, PortError> {
    u16::try_from(value).map_err(|_| corrupt_limits("a limit does not fit u16"))
}

const fn corrupt_limits(reason: &'static str) -> PortError {
    PortError::Corrupt {
        kind: "effective limits",
        reason,
    }
}

fn port_error(error: &StoreError, kind: &'static str) -> PortError {
    match error {
        StoreError::Throttled { .. } | StoreError::Contended => PortError::Throttled { kind },
        StoreError::Corrupt(_)
        | StoreError::Invalid { .. }
        | StoreError::Key(_)
        | StoreError::ItemTooLarge { .. } => PortError::Corrupt {
            kind,
            reason: "the regional authority returned malformed state",
        },
        _ => PortError::Unavailable { kind },
    }
}

/// The seam each refusal names.
/// The guest already answers a listing and a
/// stat from `lstat` alone — `aex_hands_tools::filesystem::list_dir` and
/// `stat_path`, dispatched by `runtimes/hands-agent/src/execute.rs` — with no
/// digest, no Merkle build and no TLS. What is missing is entirely on this
/// side: reaching a running generation needs the authenticated guest transport
/// (`aex_brain_hands::HttpGuestTransport`) over a provider endpoint and token
/// minted by `MicrovmControlApi::auth_token`, and this deployable composes
/// neither. Composing them here would also hand a `public_edge` service the
/// same trait that carries `run`, `resume` and `terminate`, which is a
/// lifecycle-authority decision and not a file-read one.
const LIVE_OBSERVATION_SEAM: &str = "the guest answers a listing from `lstat` with no digest; what is absent is the authenticated \
     guest transport and the provider endpoint token, which this deployable does not compose";

#[async_trait::async_trait]
impl LiveWorkspaceReader for UnownedPorts {
    async fn list(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        _query: &LiveListQuery,
    ) -> Result<LiveListing, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace listing",
            seam: LIVE_OBSERVATION_SEAM,
        })
    }

    async fn stat(
        &self,
        _session: SessionId,
        _generation: GenerationId,
        _path: &str,
    ) -> Result<LiveEntry, PortError> {
        Err(PortError::Unowned {
            kind: "live workspace entry",
            seam: LIVE_OBSERVATION_SEAM,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_wire::models::{LimitMapValue, LimitScalarValue, LimitSource};
    use aex_wire::types::{DecimalU128, Timestamp};

    fn row(id: LimitId, revision: u64) -> EffectiveWorkspaceLimit {
        let effective_value = match id {
            LimitId::SessionAgentExecution => LimitValue::Map(LimitMapValue {
                values: [
                    ("max_turns", 32),
                    ("max_steps_per_turn", 16),
                    ("turn_deadline_ms", 600_000),
                    ("max_depth", 4),
                    ("max_fanout", 32),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), DecimalU128::new(value)))
                .collect(),
            }),
            LimitId::SessionRunBudget => LimitValue::Map(LimitMapValue {
                values: [
                    ("max_run_duration_ms", 3_600_000),
                    ("total_children_created", 128),
                    ("provider_calls", 96),
                    ("hands_calls", 64),
                    ("queued_children", 32),
                    ("retained_result_bytes", 8_388_608),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), DecimalU128::new(value)))
                .collect(),
            }),
            _ if !id.dimensions().is_empty() => LimitValue::Map(LimitMapValue {
                values: id
                    .dimensions()
                    .iter()
                    .map(|name| ((*name).to_owned(), DecimalU128::new(1)))
                    .collect(),
            }),
            _ => LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(1),
            }),
        };
        EffectiveWorkspaceLimit {
            changed_at: Timestamp::from_unix_millis(1).expect("instant"),
            effective_value,
            id,
            revision,
            source: LimitSource::Default,
        }
    }

    #[test]
    fn the_revision_bound_bundle_projects_closed_execution_maps_exactly() {
        let revision = 17;
        let rows = LimitId::ALL
            .iter()
            .copied()
            .map(|id| row(id, revision))
            .collect::<Vec<_>>();
        let projected = project_limit_bundle(revision, &rows).expect("complete bundle");
        assert_eq!(projected.revision, revision);
        assert_eq!(projected.agent_execution.max_turns, 32);
        assert_eq!(projected.agent_execution.max_steps_per_turn, 16);
        assert_eq!(projected.agent_execution.turn_deadline_ms, 600_000);
        assert_eq!(projected.agent_execution.max_depth, 4);
        assert_eq!(projected.agent_execution.max_fanout, 32);
        assert_eq!(projected.run_budget.max_run_duration_ms, 3_600_000);
        assert_eq!(projected.run_budget.total_children_created, 128);
        assert_eq!(projected.run_budget.provider_calls, 96);
        assert_eq!(projected.run_budget.hands_calls, 64);
        assert_eq!(projected.run_budget.queued_children, 32);
        assert_eq!(projected.run_budget.retained_result_bytes, 8_388_608);
        assert_eq!(
            projected.limits.require(LimitId::SessionInitialFilesBytes),
            Ok(1)
        );
    }

    #[test]
    fn a_limit_bundle_revision_mismatch_is_corruption() {
        let revision = 17;
        let mut rows = LimitId::ALL
            .iter()
            .copied()
            .map(|id| row(id, revision))
            .collect::<Vec<_>>();
        rows[0].revision += 1;
        assert!(matches!(
            project_limit_bundle(revision, &rows),
            Err(PortError::Corrupt {
                kind: "effective limits",
                ..
            })
        ));
    }
}
