//! Closed direct-invoke controller for durable workspace capacity.

use aex_capacity_dynamodb::{
    AppliedCapacity, CapacityCommand, CapacityDefaults, CapacityError, CapacityState,
    CapacityStore, CapacityStoreError,
};
use aex_session_dynamodb::error::StoreError;
use aex_wire::ids::WorkspaceId;
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Required composition variables.
pub mod keys {
    /// Deployment plane.
    pub const PLANE: &str = "AEX_PLANE";
    /// Launch region.
    pub const REGION: &str = "AEX_REGION";
    /// Immutable release identity.
    pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
    /// Durable capacity authority table.
    pub const AUTHORITY_TABLE: &str = "AEX_CAPACITY_AUTHORITY_TABLE";
    /// Public effective-limit projection table.
    pub const PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
    /// Full required set, in diagnostic order.
    pub const ALL: &[&str] = &[
        PLANE,
        REGION,
        RELEASE_DIGEST,
        AUTHORITY_TABLE,
        PROJECTION_TABLE,
    ];
}

/// Why the controller refused startup.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RegionalCapacityControllerConfigError {
    /// Required input was absent or empty.
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    /// Required input had an invalid closed value.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// Variable name.
        name: &'static str,
        /// Bounded explanation.
        reason: String,
    },
}

/// Validated controller composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// `dev` or `prd`.
    pub plane: String,
    /// Pinned launch region.
    pub region: Region,
    /// Immutable source/artifact identity.
    pub release_digest: String,
    /// Durable capacity authority table.
    pub authority_table: String,
    /// Public effective-limit projection table.
    pub projection_table: String,
}

impl Config {
    /// Reads process configuration.
    ///
    /// # Errors
    ///
    /// Returns the first absent or invalid required value.
    pub fn from_env() -> Result<Self, RegionalCapacityControllerConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads a testable configuration lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Self::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, RegionalCapacityControllerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, keys::PLANE)?;
        if !matches!(plane.as_str(), "dev" | "prd") {
            return Err(invalid(keys::PLANE, "expected `dev` or `prd`"));
        }
        let region_raw = required(&lookup, keys::REGION)?;
        let region = Region::from_name(&region_raw)
            .ok_or_else(|| invalid(keys::REGION, "outside the launch-region vocabulary"))?;
        let release_digest = required(&lookup, keys::RELEASE_DIGEST)?;
        let digest = release_digest.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(
                keys::RELEASE_DIGEST,
                "expected `sha256:` followed by 64 hexadecimal digits",
            ));
        }
        let authority_table = required(&lookup, keys::AUTHORITY_TABLE)?;
        let projection_table = required(&lookup, keys::PROJECTION_TABLE)?;
        if authority_table == projection_table {
            return Err(invalid(
                keys::PROJECTION_TABLE,
                "authority and projection must be distinct tables",
            ));
        }
        Ok(Self {
            plane,
            region,
            release_digest,
            authority_table,
            projection_table,
        })
    }
}

fn required<F>(
    lookup: &F,
    name: &'static str,
) -> Result<String, RegionalCapacityControllerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(RegionalCapacityControllerConfigError::Missing(name))
}

fn invalid(name: &'static str, reason: &str) -> RegionalCapacityControllerConfigError {
    RegionalCapacityControllerConfigError::Invalid {
        name,
        reason: reason.to_owned(),
    }
}

/// One resumable page of a defaults-revision sweep.
///
/// A defaults change reaches an existing workspace only through `Reconcile`,
/// and a limit id added to the registry after a workspace was bootstrapped
/// reaches it the same way — `Bootstrap` refuses a workspace that already
/// exists, deliberately, so that a log always says which of the two happened.
/// That makes the sweep the only repair path there is, and until now nothing
/// could enumerate the workspaces to run it over.
///
/// It is explicit and on demand, never scheduled and never on a read path. The
/// defaults document is immutable per release, so a sweep with nothing to do is
/// pure cost with no signal; and a customer read that wrote an authority
/// revision would be a far worse trade than a manual step after a release.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SweepRequest {
    /// Reconcile up to `budget` workspaces, resuming after `after`.
    ReconcileSweep {
        /// Where the previous page stopped. Absent starts at the beginning.
        #[serde(default)]
        after: Option<WorkspaceId>,
        /// How many workspaces this invocation may reconcile.
        #[serde(default)]
        budget: Option<u32>,
    },
}

/// Everything this controller can be asked to do.
///
/// Untagged over the two shapes rather than one widened command enum: the
/// command vocabulary is durable — it is stored verbatim as `last_command` and
/// is what an exact-replay check compares against — so a sweep, which is an
/// operator action and not an authority transition, must not become a value the
/// authority can persist.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ControllerRequest {
    /// Walk the workspaces and reconcile each one.
    Sweep(SweepRequest),
    /// One authority command against one workspace.
    Command(CapacityCommand),
}

/// How many workspaces one sweep invocation reconciles by default.
///
/// Each one is a strong read plus a bounded cross-table transaction, against a
/// 15 s function timeout and five reserved concurrent invocations. Twenty-five
/// leaves the deadline a wide margin, and the caller resumes from `nextAfter`.
pub const DEFAULT_SWEEP_BUDGET: u32 = 25;

/// The largest budget one invocation will accept.
pub const MAX_SWEEP_BUDGET: u32 = 100;

/// What one sweep page did.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweptPage {
    /// How many workspaces were visited.
    pub visited: u32,
    /// How many of them published a new authority revision.
    pub changed: u32,
    /// Resume token. `None` means the walk reached the end.
    pub next_after: Option<WorkspaceId>,
    /// Workspaces this page could not reconcile, and why.
    ///
    /// A refusal never stops the walk. One workspace whose authority row is at a
    /// revision the sweep did not expect must not leave every workspace after it
    /// unreconciled, so the page records it and continues — and a sweep that
    /// reports refusals is visibly incomplete, which is the point.
    pub refused: Vec<SweepRefusal>,
}

/// One workspace a sweep page could not reconcile.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepRefusal {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// The controller's stable closed reason code.
    pub code: &'static str,
}

/// The bounded JSON returned to an internal direct invoker.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControllerResponse {
    /// A new revision committed, or an exact command replay resolved.
    Applied {
        /// Complete durable state.
        state: Box<CapacityState>,
        /// False for an exact durable replay or no-op reconcile.
        changed: bool,
    },
    /// A deterministic policy/precondition refusal; no write occurred.
    Refused {
        /// Stable closed reason code.
        code: &'static str,
        /// Bounded human diagnostic.
        message: String,
    },
    /// One page of a defaults-revision sweep completed.
    Swept(SweptPage),
}

/// Clock boundary used by the deterministic controller tests.
pub trait Clock: Send + Sync {
    /// Current millisecond-truncated instant.
    fn now(&self) -> Timestamp;
}

/// Production wall clock.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .expect("the host clock must fit the public timestamp range")
    }
}

/// Narrow durable store boundary.
#[async_trait]
pub trait Store: Send + Sync {
    /// Applies one complete command.
    async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError>;

    /// Reads one workspace's durable authority, for the revision a sweep must
    /// reconcile against.
    async fn load(&self, workspace: WorkspaceId) -> Result<Option<CapacityState>, StoreError>;

    /// Enumerates the workspaces this region holds an authority row for.
    async fn page_workspaces(
        &self,
        budget: u32,
        after: Option<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, StoreError>;
}

#[async_trait]
impl Store for CapacityStore {
    async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError> {
        CapacityStore::apply(self, defaults, command, now).await
    }

    async fn load(&self, workspace: WorkspaceId) -> Result<Option<CapacityState>, StoreError> {
        CapacityStore::load(self, workspace).await
    }

    async fn page_workspaces(
        &self,
        budget: u32,
        after: Option<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, StoreError> {
        CapacityStore::page_workspaces(self, budget, after).await
    }
}

/// Stateless Lambda handler around the durable authority.
#[derive(Debug)]
pub struct CapacityController<S, C> {
    store: S,
    defaults: CapacityDefaults,
    clock: C,
}

impl<S, C> CapacityController<S, C>
where
    S: Store,
    C: Clock,
{
    /// Creates a handler from validated defaults and explicit boundaries.
    #[must_use]
    pub const fn new(store: S, defaults: CapacityDefaults, clock: C) -> Self {
        Self {
            store,
            defaults,
            clock,
        }
    }

    /// Handles one closed [`CapacityCommand`] direct invocation.
    ///
    /// Deterministic policy errors are returned as bounded JSON so callers do
    /// not retry them. Store/transport errors become Lambda function errors so
    /// the invoker's bounded retry/DLQ policy remains authoritative.
    ///
    /// # Errors
    ///
    /// Returns only operational persistence failures.
    pub async fn handle(
        &self,
        request: ControllerRequest,
    ) -> Result<ControllerResponse, lambda_runtime::Error> {
        match request {
            ControllerRequest::Command(command) => self.command(command).await,
            ControllerRequest::Sweep(SweepRequest::ReconcileSweep { after, budget }) => {
                self.sweep(after, budget).await
            }
        }
    }

    /// Walks one page of workspaces, reconciling each to the embedded defaults.
    ///
    /// Each workspace is read strongly and then reconciled against the exact
    /// revision that read returned. A workspace already at the current defaults
    /// revision and holding the complete registry is a no-op, so a sweep run
    /// twice costs two reads and writes nothing.
    ///
    /// # Errors
    ///
    /// Returns only operational persistence failures. A per-workspace policy
    /// refusal is recorded on the page and the walk continues, because one
    /// workspace at an unexpected revision must not leave every workspace after
    /// it unreconciled.
    async fn sweep(
        &self,
        after: Option<WorkspaceId>,
        budget: Option<u32>,
    ) -> Result<ControllerResponse, lambda_runtime::Error> {
        let budget = budget
            .unwrap_or(DEFAULT_SWEEP_BUDGET)
            .clamp(1, MAX_SWEEP_BUDGET);
        let workspaces = self.store.page_workspaces(budget, after).await?;
        let mut page = SweptPage {
            visited: 0,
            changed: 0,
            next_after: None,
            refused: Vec::new(),
        };
        for workspace in workspaces {
            page.visited = page.visited.saturating_add(1);
            page.next_after = Some(workspace);
            let Some(state) = self.store.load(workspace).await? else {
                // Enumerated and then gone. A workspace deleted between the
                // index read and the authority read is not a refusal and not a
                // repair; it is simply no longer here.
                continue;
            };
            let command = CapacityCommand::Reconcile {
                workspace_id: workspace,
                expected_revision: state.revision,
            };
            match self
                .store
                .apply(&self.defaults, &command, self.clock.now())
                .await
            {
                Ok(AppliedCapacity { changed: true, .. }) => {
                    page.changed = page.changed.saturating_add(1);
                }
                Ok(AppliedCapacity { changed: false, .. }) => {}
                Err(CapacityStoreError::Capacity(error)) => page.refused.push(SweepRefusal {
                    workspace,
                    code: capacity_code(&error),
                }),
                Err(CapacityStoreError::Store(error)) => return Err(error.into()),
            }
        }
        // A short page is the end of the walk. Reporting a resume token for it
        // would make an operator run one more empty page every time, and a
        // sweep that never reports completion is one nobody can tell is done.
        if page.visited < budget {
            page.next_after = None;
        }
        Ok(ControllerResponse::Swept(page))
    }

    /// Handles one closed [`CapacityCommand`].
    ///
    /// # Errors
    ///
    /// Returns only operational persistence failures.
    async fn command(
        &self,
        command: CapacityCommand,
    ) -> Result<ControllerResponse, lambda_runtime::Error> {
        match self
            .store
            .apply(&self.defaults, &command, self.clock.now())
            .await
        {
            Ok(AppliedCapacity { state, changed }) => Ok(ControllerResponse::Applied {
                state: Box::new(state),
                changed,
            }),
            Err(CapacityStoreError::Capacity(error)) => Ok(ControllerResponse::Refused {
                code: capacity_code(&error),
                message: bounded_message(&error.to_string()),
            }),
            Err(CapacityStoreError::Store(error)) => Err(error.into()),
        }
    }
}

fn capacity_code(error: &CapacityError) -> &'static str {
    match error {
        CapacityError::AlreadyExists => "already_exists",
        CapacityError::Missing => "missing",
        CapacityError::Revision { .. } => "revision_conflict",
        CapacityError::CapacityFence { .. } => "capacity_fence_conflict",
        CapacityError::Approval => "invalid_approval",
        CapacityError::DefaultsRegression { .. } => "defaults_regression",
        CapacityError::DefaultsDigest { .. } => "defaults_digest_conflict",
        CapacityError::Override { .. } => "invalid_override",
        CapacityError::RevisionExhausted => "revision_exhausted",
    }
}

fn bounded_message(message: &str) -> String {
    message.chars().take(256).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use aex_capacity_dynamodb::{
        CapacityCommand, CapacityStoreError, canonical_defaults, plan_capacity_change,
    };
    use aex_session_dynamodb::error::StoreError;
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitScalarValue, LimitValue};
    use aex_wire::types::{DecimalU128, Timestamp};
    use async_trait::async_trait;

    use super::{
        CapacityController, Clock, ControllerRequest, ControllerResponse, MAX_SWEEP_BUDGET, Store,
    };

    /// Every workspace this fake region holds, keyed the way the index orders
    /// them, so a sweep's resume token behaves the way the real query does.
    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<WorkspaceId, aex_capacity_dynamodb::CapacityState>>);

    impl MemoryStore {
        fn with(state: Option<aex_capacity_dynamodb::CapacityState>) -> Self {
            Self(Mutex::new(
                state
                    .map(|state| (state.workspace_id, state))
                    .into_iter()
                    .collect(),
            ))
        }
    }

    #[async_trait]
    impl Store for MemoryStore {
        async fn apply(
            &self,
            defaults: &aex_capacity_dynamodb::CapacityDefaults,
            command: &CapacityCommand,
            now: Timestamp,
        ) -> Result<aex_capacity_dynamodb::AppliedCapacity, CapacityStoreError> {
            let mut rows = self.0.lock().expect("lock");
            let current = rows.get(&command.workspace()).cloned();
            let planned = plan_capacity_change(current.as_ref(), defaults, command, now)?;
            if planned.changed {
                rows.insert(planned.state.workspace_id, planned.state.clone());
            }
            Ok(aex_capacity_dynamodb::AppliedCapacity {
                state: planned.state,
                changed: planned.changed,
            })
        }

        async fn load(
            &self,
            workspace: WorkspaceId,
        ) -> Result<Option<aex_capacity_dynamodb::CapacityState>, StoreError> {
            Ok(self.0.lock().expect("lock").get(&workspace).cloned())
        }

        async fn page_workspaces(
            &self,
            budget: u32,
            after: Option<WorkspaceId>,
        ) -> Result<Vec<WorkspaceId>, StoreError> {
            Ok(self
                .0
                .lock()
                .expect("lock")
                .keys()
                .copied()
                .filter(|id| after.is_none_or(|after| *id > after))
                .take(budget as usize)
                .collect())
        }
    }

    #[derive(Clone, Copy)]
    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10]))
    }

    fn controller() -> CapacityController<MemoryStore, FixedClock> {
        CapacityController::new(
            MemoryStore::with(None),
            canonical_defaults().expect("defaults"),
            FixedClock(Timestamp::from_unix_millis(1).expect("timestamp")),
        )
    }

    /// The command envelope, so a case reads as the invoke it stands for.
    fn command(command: CapacityCommand) -> ControllerRequest {
        ControllerRequest::Command(command)
    }

    fn sweep(after: Option<WorkspaceId>, budget: Option<u32>) -> ControllerRequest {
        ControllerRequest::Sweep(super::SweepRequest::ReconcileSweep { after, budget })
    }

    #[tokio::test]
    async fn bootstrap_reconcile_override_and_exact_retry_are_closed() {
        let controller = controller();
        let bootstrap = CapacityCommand::Bootstrap {
            workspace_id: workspace(),
        };
        assert!(matches!(
            controller
                .handle(command(bootstrap.clone()))
                .await
                .expect("bootstrap"),
            ControllerResponse::Applied { changed: true, .. }
        ));
        assert!(matches!(
            controller.handle(command(bootstrap)).await.expect("replay"),
            ControllerResponse::Applied { changed: false, .. }
        ));
        assert!(matches!(
            controller
                .handle(command(CapacityCommand::Reconcile {
                    workspace_id: workspace(),
                    expected_revision: 1,
                }))
                .await
                .expect("reconcile"),
            ControllerResponse::Applied { changed: false, .. }
        ));
        let override_command = CapacityCommand::SetOverride {
            workspace_id: workspace(),
            expected_revision: 1,
            capacity_fence: 7,
            approval_id: "support-123".to_owned(),
            limit_id: LimitId::ApiJsonBody,
            value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(131_072),
            }),
        };
        assert!(matches!(
            controller
                .handle(command(override_command.clone()))
                .await
                .expect("override"),
            ControllerResponse::Applied { changed: true, .. }
        ));
        assert!(matches!(
            controller
                .handle(command(override_command))
                .await
                .expect("exact retry"),
            ControllerResponse::Applied { changed: false, .. }
        ));
    }

    #[tokio::test]
    async fn malformed_approval_and_stale_revision_are_bounded_refusals() {
        let controller = controller();
        controller
            .handle(command(CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            }))
            .await
            .expect("bootstrap");
        let response = controller
            .handle(command(CapacityCommand::SetOverride {
                workspace_id: workspace(),
                expected_revision: 1,
                capacity_fence: 1,
                approval_id: String::new(),
                limit_id: LimitId::ApiJsonBody,
                value: LimitValue::Scalar(LimitScalarValue {
                    value: DecimalU128::new(1),
                }),
            }))
            .await
            .expect("deterministic refusal");
        assert!(matches!(
            response,
            ControllerResponse::Refused {
                code: "invalid_approval",
                ..
            }
        ));
        let encoded = serde_json::to_vec(&response).expect("JSON");
        assert!(encoded.len() < 512);
    }

    /// The registry grew after these workspaces were bootstrapped, and one
    /// sweep repairs every one of them.
    ///
    /// This is the whole reason the sweep exists. `Bootstrap` refuses a
    /// workspace that already has an authority row — deliberately, so a log
    /// always says which of the two happened — so a limit id added to the
    /// registry after a workspace was provisioned reaches it through
    /// `Reconcile` and through nothing else. Until the enumeration existed,
    /// nothing could run that reconcile over the workspaces that needed it.
    #[tokio::test]
    async fn a_sweep_repairs_every_workspace_the_registry_grew_past() {
        let controller = controller();
        let workspaces: Vec<WorkspaceId> = (1_u8..=3)
            .map(|seed| WorkspaceId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10])))
            .collect();
        for id in &workspaces {
            controller
                .handle(command(CapacityCommand::Bootstrap { workspace_id: *id }))
                .await
                .expect("bootstrap");
        }

        // Exactly the shape of a workspace bootstrapped before a limit id
        // existed: complete for the registry it saw, short for the one it did
        // not, at an unchanged defaults revision.
        let dropped = *LimitId::ALL.last().expect("a non-empty registry");
        {
            let mut rows = controller.store.0.lock().expect("lock");
            for id in &workspaces {
                rows.get_mut(id)
                    .expect("a bootstrapped row")
                    .effective
                    .remove(&dropped);
            }
        }

        let ControllerResponse::Swept(page) = controller
            .handle(sweep(None, None))
            .await
            .expect("a sweep runs")
        else {
            panic!("a sweep answered a command response");
        };
        assert_eq!(page.visited, 3);
        assert_eq!(page.changed, 3, "a short workspace was left short");
        assert!(page.refused.is_empty(), "{:?}", page.refused);
        assert_eq!(page.next_after, None, "a short page is the end of the walk");

        let rows = controller.store.0.lock().expect("lock");
        for id in &workspaces {
            let state = rows.get(id).expect("a swept row");
            assert_eq!(state.effective.len(), LimitId::ALL.len(), "{id}");
            assert!(state.effective.contains_key(&dropped), "{id}");
        }
    }

    /// A second sweep writes nothing. The defaults document is immutable per
    /// release, so a sweep with nothing to do must cost two reads per workspace
    /// and no revision at all — otherwise running it twice would look like a
    /// change and every workspace's revision would drift on operator habit.
    #[tokio::test]
    async fn a_sweep_over_an_already_reconciled_region_writes_nothing() {
        let controller = controller();
        controller
            .handle(command(CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            }))
            .await
            .expect("bootstrap");
        let revision = controller
            .store
            .0
            .lock()
            .expect("lock")
            .get(&workspace())
            .expect("a row")
            .revision;

        for _ in 0..2 {
            let ControllerResponse::Swept(page) = controller
                .handle(sweep(None, None))
                .await
                .expect("a sweep runs")
            else {
                panic!("a sweep answered a command response");
            };
            assert_eq!(page.visited, 1);
            assert_eq!(page.changed, 0, "an idempotent sweep published a revision");
        }
        assert_eq!(
            controller
                .store
                .0
                .lock()
                .expect("lock")
                .get(&workspace())
                .expect("a row")
                .revision,
            revision,
            "the authority revision moved on a sweep that had nothing to do"
        );
    }

    /// The walk is resumable and its budget is bounded.
    ///
    /// A sweep runs against a 15 s function timeout, so it cannot be one
    /// invocation over a whole region. It has to stop, say where, and be
    /// restartable there without revisiting or skipping a workspace.
    #[tokio::test]
    async fn a_sweep_resumes_exactly_where_it_stopped() {
        let controller = controller();
        let mut workspaces: Vec<WorkspaceId> = (1_u8..=5)
            .map(|seed| WorkspaceId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10])))
            .collect();
        workspaces.sort_unstable();
        for id in &workspaces {
            controller
                .handle(command(CapacityCommand::Bootstrap { workspace_id: *id }))
                .await
                .expect("bootstrap");
        }

        let mut seen = Vec::new();
        let mut after = None;
        loop {
            let ControllerResponse::Swept(page) = controller
                .handle(sweep(after, Some(2)))
                .await
                .expect("a sweep runs")
            else {
                panic!("a sweep answered a command response");
            };
            assert!(page.visited <= 2, "the budget was exceeded");
            seen.push(page.visited);
            let Some(next) = page.next_after else { break };
            after = Some(next);
        }
        assert_eq!(
            seen,
            vec![2, 2, 1],
            "the walk did not page the way its budget says"
        );

        // And the budget is clamped rather than trusted: an operator asking for
        // a region-sized page would otherwise be asking for a timeout.
        let ControllerResponse::Swept(page) = controller
            .handle(sweep(None, Some(MAX_SWEEP_BUDGET * 100)))
            .await
            .expect("a sweep runs")
        else {
            panic!("a sweep answered a command response");
        };
        assert_eq!(page.visited, 5);
    }

    /// One workspace at an unexpected revision does not end the walk.
    ///
    /// A sweep that stopped at the first refusal would leave every workspace
    /// after it unreconciled, and the operator would see a partial success that
    /// looked like a whole one. The refusal is recorded and the walk continues.
    #[tokio::test]
    async fn a_refused_workspace_is_recorded_and_the_walk_continues() {
        let controller = CapacityController::new(
            MemoryStore::default(),
            canonical_defaults().expect("defaults"),
            FixedClock(Timestamp::from_unix_millis(1).expect("timestamp")),
        );
        let workspaces: Vec<WorkspaceId> = (1_u8..=3)
            .map(|seed| WorkspaceId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10])))
            .collect();
        for id in &workspaces {
            controller
                .handle(command(CapacityCommand::Bootstrap { workspace_id: *id }))
                .await
                .expect("bootstrap");
        }
        // A stored digest that disagrees with the running artifact's is the
        // refusal an edited-in-place defaults document produces. It is per
        // workspace, and every other workspace in the region is still fine.
        {
            let mut rows = controller.store.0.lock().expect("lock");
            let poisoned = rows.get_mut(&workspaces[1]).expect("a bootstrapped row");
            poisoned.defaults_digest = format!("sha256:{}", "f".repeat(64));
        }

        let ControllerResponse::Swept(page) = controller
            .handle(sweep(None, None))
            .await
            .expect("a sweep runs")
        else {
            panic!("a sweep answered a command response");
        };
        assert_eq!(page.visited, 3, "the walk stopped at the refusal");
        assert_eq!(page.refused.len(), 1);
        assert_eq!(page.refused[0].workspace, workspaces[1]);
        assert_eq!(page.refused[0].code, "defaults_digest_conflict");
    }

    /// A sweep is an operator action and never becomes a durable command.
    ///
    /// `last_command` is compared verbatim by the exact-replay check, so a
    /// request shape that could be stored there would make a replay decision
    /// depend on how an operator happened to invoke the controller.
    #[test]
    fn the_two_request_shapes_decode_without_shadowing_each_other() {
        let sweep: ControllerRequest =
            serde_json::from_value(serde_json::json!({ "kind": "reconcile_sweep" }))
                .expect("a sweep decodes");
        assert_eq!(sweep, self::sweep(None, None));

        let bootstrap: ControllerRequest = serde_json::from_value(
            serde_json::json!({ "kind": "bootstrap", "workspace_id": workspace().to_string() }),
        )
        .expect("a command decodes");
        assert_eq!(
            bootstrap,
            command(CapacityCommand::Bootstrap {
                workspace_id: workspace()
            }),
            "a command was swallowed by the sweep arm"
        );

        // An unknown verb is refused rather than falling through to either arm.
        assert!(
            serde_json::from_value::<ControllerRequest>(
                serde_json::json!({ "kind": "delete_everything" })
            )
            .is_err()
        );
    }
}
