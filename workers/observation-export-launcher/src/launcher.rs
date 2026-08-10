//! The due-scan, the durable claim and the identity-reconciled `ECS` launch.
//!
//! # The one invariant
//!
//! **An export is launched at most once.** Two mechanisms hold it, and neither
//! is a retry:
//!
//! 1. A conditional `UpdateItem` fences the claim. Losing that condition is a
//!    normal outcome — the item is skipped, never retried into a second launch.
//! 2. The only unambiguous `RunTask` result is one that *names* the task it
//!    started. Every other result — a refusal, a partial failure, a transport
//!    error — is reconciled by **identity**: `ListTasks` filtered by
//!    `startedBy = {export id}` across `RUNNING` and `STOPPED`. A sweep issues
//!    at most one `RunTask` per claimed export, so an unknown outcome that the
//!    reconciliation cannot resolve is reported as unresolved and left for the
//!    next due window rather than relaunched.
//!
//! The two `AWS` surfaces sit behind [`ExportRows`] and [`TaskLauncher`] so the
//! reconciliation above is exercised against a hand-written fake rather than
//! against a live cluster.

use std::time::Duration;

use aex_observation_domain::keys::{ControlDomain, control_pk, control_sk, export_pk, export_sk};
use aex_wire::ids::{ExportId, PrefixedId, WorkspaceId};
use aex_wire::types::Timestamp;

/// The control domain this launcher scans.
pub const DOMAIN: ControlDomain = ControlDomain::ExportLaunch;

// There is no constant sort key for the export state row any more. A row is
// keyed `EXPORT#{workspace}` / `{export_id}` and `DueItem::state_sk` derives
// it; the former `EXPORT_STATE_SK` put every workspace's exports in their own
// partition.

/// The state an admitted, not yet launched export carries.
pub const STATE_ADMITTED: &str = "admitted";

/// The state a claimed export carries while its task is being started.
pub const STATE_LAUNCHING: &str = "launching";

/// The environment variable the export task reads its export id from.
pub const EXPORT_ID_ENV: &str = "AEX_EXPORT_ID";

/// The environment variable the export task reads its workspace id from.
pub const WORKSPACE_ID_ENV: &str = "AEX_WORKSPACE_ID";

/// The task statuses an identity reconciliation covers.
///
/// A task that started and has already stopped is still a task that started, so
/// omitting `STOPPED` would relaunch exactly the export that finished fastest.
pub const RECONCILED_STATUSES: [TaskStatus; 2] = [TaskStatus::Running, TaskStatus::Stopped];

/// A desired task status an identity reconciliation asks about.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TaskStatus {
    /// The task has not stopped.
    Running,
    /// The task has stopped, successfully or otherwise.
    Stopped,
}

impl TaskStatus {
    /// The `ECS` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Stopped => "STOPPED",
        }
    }
}

/// Why the launcher could not complete a step.
#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    /// The export authority refused a read or a write.
    #[error("the export authority refused a {operation}: {reason}")]
    Rows {
        /// Which step failed.
        operation: &'static str,
        /// What the authority reported.
        reason: String,
    },
    /// The export cluster refused a call.
    #[error("the export cluster refused a {operation}: {reason}")]
    Tasks {
        /// Which call failed.
        operation: &'static str,
        /// What the cluster reported.
        reason: String,
    },
    /// A stored key did not match the template it is written under.
    #[error("`{key}` is not an export control key: {reason}")]
    Key {
        /// The offending key.
        key: String,
        /// Why it was rejected.
        reason: String,
    },
    /// An instant could not be represented in the wire spelling.
    #[error("{millis} is not a representable instant")]
    Instant {
        /// The offending value.
        millis: i64,
    },
}

/// One due `export.launch` control item.
///
/// The sparse control index is `KEYS_ONLY`, so this is everything a due-scan can
/// possibly learn — which is the point: the launcher holds no observation read
/// permission and has no way to see anything else.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DueItem {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The export to launch.
    pub export: ExportId,
    /// When the item became due.
    pub due_at: Timestamp,
}

impl DueItem {
    /// The `EXPORT#{workspace}` partition key of the state row.
    #[must_use]
    pub fn state_pk(&self) -> String {
        export_pk(self.workspace)
    }

    /// The sort key of the state row: the bare export identity.
    #[must_use]
    pub fn state_sk(&self) -> String {
        export_sk(self.export)
    }
}

/// The lease one sweep claims its exports under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lease {
    /// Who holds the claim. The invocation's own request id: a real identity,
    /// not a generated one, so a stuck claim can be traced to its invocation.
    pub owner: String,
    /// The instant the claim is evaluated at.
    pub now: Timestamp,
    /// When the claim stops being honoured.
    pub expires_at: Timestamp,
}

impl Lease {
    /// Builds the lease one sweep runs under.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Instant`] when `now + duration` falls outside
    /// the representable wire range.
    pub fn new(owner: &str, now: Timestamp, duration: Duration) -> Result<Self, LauncherError> {
        let millis = i64::try_from(duration.as_millis())
            .ok()
            .and_then(|span| now.unix_millis().checked_add(span))
            .ok_or(LauncherError::Instant {
                millis: now.unix_millis(),
            })?;
        let expires_at =
            Timestamp::from_unix_millis(millis).map_err(|_| LauncherError::Instant { millis })?;
        Ok(Self {
            owner: owner.to_owned(),
            now,
            expires_at,
        })
    }
}

/// The fence a won claim holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    /// The monotonic fence the claim advanced the row to.
    pub fence: u64,
}

/// What a conditional claim did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimOutcome {
    /// The condition held; this process owns the launch.
    Won(Claim),
    /// The condition did not hold. Normal, not exceptional: another launcher
    /// owns the export, the lease is still live, or a cancel was requested.
    Lost,
}

/// What recording the task identity did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordOutcome {
    /// The identity was written under the fence this process holds.
    Recorded,
    /// The fence advanced while the task was starting. The launch itself still
    /// happened exactly once, because the export id is the task's identity.
    Superseded,
}

/// What one `RunTask` call reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTaskOutcome {
    /// The call named the task it started. The only unambiguous result.
    Started {
        /// The started task's `ARN`.
        task_arn: String,
    },
    /// The call reported a failure for the placement it was asked for.
    Refused {
        /// What the cluster reported.
        reason: String,
    },
    /// The call answered without naming a task and without a placement failure.
    Unknown {
        /// What was observed instead.
        reason: String,
    },
}

/// How one claimed export's launch resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchResolution {
    /// A task exists for this export, either because `RunTask` named it or
    /// because the identity reconciliation found it.
    Launched {
        /// The task's `ARN`.
        task_arn: String,
    },
    /// No task could be attributed to this export. The claim's lease expires and
    /// a later sweep retries the *claim*; this sweep does not retry the launch.
    Unresolved {
        /// Why the outcome was ambiguous.
        reason: String,
    },
}

/// Everything one `RunTask` needs.
#[derive(Clone, Copy, Debug)]
pub struct LaunchRequest<'a> {
    /// The cluster `ARN` the task is placed in.
    pub cluster: &'a str,
    /// The task-definition `ARN` to run.
    pub task_definition: &'a str,
    /// The container the overrides are addressed to.
    pub container: &'a str,
    /// The subnets the task is placed in.
    pub subnets: &'a [String],
    /// The security groups the task runs under.
    pub security_groups: &'a [String],
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The export being produced.
    pub export: ExportId,
}

impl LaunchRequest<'_> {
    /// The idempotency token and `startedBy` marker: the export's own identity.
    ///
    /// Using one value for both is what makes an ambiguous outcome recoverable —
    /// the launcher can always ask the cluster "did *this export* ever start?"
    /// without having kept anything of its own.
    #[must_use]
    pub fn identity(&self) -> String {
        self.export.to_string()
    }

    /// The container environment overrides: exactly the two identifiers the
    /// export task needs to find its own work, and nothing else.
    ///
    /// No credential, no table name and no bucket travels this way. The task
    /// resolves those from its own configuration under its own role.
    #[must_use]
    pub fn overrides(&self) -> [(&'static str, String); 2] {
        [
            (EXPORT_ID_ENV, self.export.to_string()),
            (WORKSPACE_ID_ENV, self.workspace.to_string()),
        ]
    }
}

/// The `EXPORT#` and `CTRL#` rows the launcher may touch.
#[async_trait::async_trait]
pub trait ExportRows: Send + Sync {
    /// Reads the due `export.launch` items of one control shard.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Rows`] when the query fails and
    /// [`LauncherError::Key`] when a stored key does not match its template.
    async fn due(
        &self,
        shard: u8,
        now: Timestamp,
        limit: usize,
    ) -> Result<Vec<DueItem>, LauncherError>;

    /// Claims one export for launch under a lease.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Rows`] when the write fails for any reason other
    /// than the condition, which is [`ClaimOutcome::Lost`] rather than an error.
    async fn claim(&self, item: &DueItem, lease: &Lease) -> Result<ClaimOutcome, LauncherError>;

    /// Records the task identity a launch resolved to.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Rows`] when the write fails for any reason other
    /// than an advanced fence, which is [`RecordOutcome::Superseded`].
    async fn record_task(
        &self,
        item: &DueItem,
        claim: Claim,
        task_arn: &str,
    ) -> Result<RecordOutcome, LauncherError>;
}

/// The `ECS` surface the launcher may touch.
#[async_trait::async_trait]
pub trait TaskLauncher: Send + Sync {
    /// Starts one export task.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Tasks`] when the call could not be made at all.
    /// A call that was made and answered ambiguously is a
    /// [`RunTaskOutcome::Unknown`], not an error, because the two are recovered
    /// differently.
    async fn run_task(&self, request: &LaunchRequest<'_>) -> Result<RunTaskOutcome, LauncherError>;

    /// Lists the tasks one `startedBy` marker owns in one desired status.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Tasks`] when the call fails. A reconciliation
    /// that cannot be completed never falls back to relaunching.
    async fn tasks_started_by(
        &self,
        cluster: &str,
        started_by: &str,
        status: TaskStatus,
    ) -> Result<Vec<String>, LauncherError>;
}

/// The bounds and placement one launcher process runs under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchSettings {
    /// The cluster `ARN` tasks are placed in.
    pub cluster: String,
    /// The task-definition `ARN` to run.
    pub task_definition: String,
    /// The container the overrides are addressed to.
    pub container: String,
    /// The subnets a task is placed in.
    pub subnets: Vec<String>,
    /// The security groups a task runs under.
    pub security_groups: Vec<String>,
    /// How many control shards one sweep scans.
    pub shards: u8,
    /// How many exports one sweep may launch.
    pub max_concurrent: usize,
    /// How long a claim is leased for.
    pub lease: Duration,
}

/// What one sweep did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepReport {
    /// Due items considered.
    pub scanned: usize,
    /// Claims won.
    pub claimed: usize,
    /// Launches resolved to a task identity.
    pub launched: usize,
    /// Claims lost, and therefore skipped without a launch.
    pub skipped: usize,
    /// Launches whose outcome the identity reconciliation could not resolve.
    pub unresolved: usize,
    /// Launches whose task identity another fence had already superseded.
    pub superseded: usize,
}

/// The composed launcher.
pub struct Launcher<R, T> {
    rows: R,
    tasks: T,
    settings: LaunchSettings,
}

impl<R, T> Launcher<R, T>
where
    R: ExportRows,
    T: TaskLauncher,
{
    /// Composes the launcher over its two ports.
    #[must_use]
    pub const fn new(rows: R, tasks: T, settings: LaunchSettings) -> Self {
        Self {
            rows,
            tasks,
            settings,
        }
    }

    /// Scans every shard for due exports, claims what it can and launches each
    /// claim exactly once.
    ///
    /// # Errors
    ///
    /// Returns the first [`LauncherError`] any port raised. A sweep that cannot
    /// read, claim or reconcile stops; it never launches on a guess.
    pub async fn sweep(&self, owner: &str, now: Timestamp) -> Result<SweepReport, LauncherError> {
        let lease = Lease::new(owner, now, self.settings.lease)?;
        let mut report = SweepReport::default();
        for shard in 0..self.settings.shards {
            let remaining = self.settings.max_concurrent.saturating_sub(report.scanned);
            if remaining == 0 {
                break;
            }
            for item in self.rows.due(shard, now, remaining).await? {
                report.scanned += 1;
                self.advance(&item, &lease, &mut report).await?;
                if report.scanned >= self.settings.max_concurrent {
                    break;
                }
            }
        }
        Ok(report)
    }

    /// Claims one due item and, if the claim held, launches it.
    async fn advance(
        &self,
        item: &DueItem,
        lease: &Lease,
        report: &mut SweepReport,
    ) -> Result<(), LauncherError> {
        let claim = match self.rows.claim(item, lease).await? {
            ClaimOutcome::Lost => {
                report.skipped += 1;
                return Ok(());
            }
            ClaimOutcome::Won(claim) => claim,
        };
        report.claimed += 1;
        tracing::debug!(
            export = %item.export,
            due_at = %item.due_at.to_wire(),
            fence = claim.fence,
            "export claimed for launch"
        );
        match self.launch(item).await? {
            LaunchResolution::Unresolved { reason } => {
                tracing::warn!(
                    export = %item.export,
                    %reason,
                    "the launch outcome stayed ambiguous; leaving it for the next due window"
                );
                report.unresolved += 1;
            }
            LaunchResolution::Launched { task_arn } => {
                match self.rows.record_task(item, claim, &task_arn).await? {
                    RecordOutcome::Recorded => report.launched += 1,
                    RecordOutcome::Superseded => report.superseded += 1,
                }
            }
        }
        Ok(())
    }

    /// Starts one export task, reconciling by identity when the outcome is not
    /// an explicitly named task.
    async fn launch(&self, item: &DueItem) -> Result<LaunchResolution, LauncherError> {
        let request = LaunchRequest {
            cluster: &self.settings.cluster,
            task_definition: &self.settings.task_definition,
            container: &self.settings.container,
            subnets: &self.settings.subnets,
            security_groups: &self.settings.security_groups,
            workspace: item.workspace,
            export: item.export,
        };
        // The single `RunTask` call site of the whole deployable.
        let reason = match self.tasks.run_task(&request).await {
            Ok(RunTaskOutcome::Started { task_arn }) => {
                return Ok(LaunchResolution::Launched { task_arn });
            }
            Ok(RunTaskOutcome::Refused { reason } | RunTaskOutcome::Unknown { reason }) => reason,
            Err(error) => error.to_string(),
        };
        self.reconcile(&request, reason).await
    }

    /// Resolves an ambiguous launch from the cluster's own task identities.
    ///
    /// No second `RunTask` is issued here or anywhere below it. An export whose
    /// task cannot be found is reported unresolved: the claim's lease expires
    /// and a later sweep re-claims it, which is one launch attempt per lease.
    async fn reconcile(
        &self,
        request: &LaunchRequest<'_>,
        reason: String,
    ) -> Result<LaunchResolution, LauncherError> {
        let identity = request.identity();
        for status in RECONCILED_STATUSES {
            let found = self
                .tasks
                .tasks_started_by(request.cluster, &identity, status)
                .await?;
            if let Some(task_arn) = found.into_iter().next() {
                return Ok(LaunchResolution::Launched { task_arn });
            }
        }
        Ok(LaunchResolution::Unresolved { reason })
    }
}

/// The `gsi_control` partition one shard's due-scan reads.
#[must_use]
pub fn shard_partition(shard: u8) -> String {
    control_pk(DOMAIN, shard)
}

/// The exclusive upper bound of the due window at `now`.
///
/// Sort keys are `{due_at}#{item_id}`, so everything due at or before `now`
/// sorts strictly below the empty-item-id key of the next millisecond. That
/// keeps the range read exact without a sentinel character.
///
/// # Errors
///
/// Returns [`LauncherError::Instant`] when `now` is the last representable
/// millisecond.
pub fn due_upper_bound(now: Timestamp) -> Result<String, LauncherError> {
    let millis = now
        .unix_millis()
        .checked_add(1)
        .ok_or(LauncherError::Instant {
            millis: now.unix_millis(),
        })?;
    let ceiling =
        Timestamp::from_unix_millis(millis).map_err(|_| LauncherError::Instant { millis })?;
    Ok(control_sk(ceiling, ""))
}

/// Splits an `EXPORT#{workspace}` partition key and its `{export}` sort key.
///
/// The two are parsed together because the due index is `KEYS_ONLY`: a due page
/// hands over exactly this pair and nothing else, which is what keeps the
/// launcher's grant free of any observation read.
///
/// # Errors
///
/// Returns [`LauncherError::Key`] when the key does not match the template or
/// either identifier does not parse as the kind its position declares.
pub fn parse_export_key(pk: &str, sk: &str) -> Result<(WorkspaceId, ExportId), LauncherError> {
    let malformed = |reason: &str| LauncherError::Key {
        key: format!("{pk}/{sk}"),
        reason: reason.to_owned(),
    };
    let workspace = pk
        .strip_prefix("EXPORT#")
        .ok_or_else(|| malformed("the key does not begin with `EXPORT#`"))?;
    if workspace.contains('#') {
        return Err(malformed(
            "the partition carries more components than the template",
        ));
    }
    let workspace =
        WorkspaceId::parse(workspace).map_err(|_| malformed("the workspace does not parse"))?;
    let export = ExportId::parse(sk).map_err(|_| malformed("the export does not parse"))?;
    Ok((workspace, export))
}

/// Splits the `{due_at}#{item_id}` sort key of a control item.
///
/// # Errors
///
/// Returns [`LauncherError::Key`] when the instant does not parse.
pub fn parse_control_sk(sk: &str) -> Result<Timestamp, LauncherError> {
    let (due_at, _item) = sk.split_once('#').ok_or_else(|| LauncherError::Key {
        key: sk.to_owned(),
        reason: "the key carries no item component".to_owned(),
    })?;
    Timestamp::parse(due_at).map_err(|_| LauncherError::Key {
        key: sk.to_owned(),
        reason: "the due instant does not parse".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_wire::ids::{ExportId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        Claim, ClaimOutcome, DueItem, EXPORT_ID_ENV, ExportRows, LaunchRequest, LaunchSettings,
        Launcher, LauncherError, Lease, RECONCILED_STATUSES, RecordOutcome, RunTaskOutcome,
        STATE_ADMITTED, STATE_LAUNCHING, TaskLauncher, TaskStatus, WORKSPACE_ID_ENV,
        due_upper_bound, parse_control_sk, parse_export_key, shard_partition,
    };

    /// The instant every case sweeps at.
    fn now() -> Timestamp {
        Timestamp::from_unix_millis(1_760_000_000_000).expect("representable")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn export(seed: u8) -> ExportId {
        ExportId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
    }

    fn due(seed: u8) -> DueItem {
        DueItem {
            workspace: workspace(),
            export: export(seed),
            due_at: now(),
        }
    }

    fn settings() -> LaunchSettings {
        LaunchSettings {
            cluster: "arn:aws:ecs:eu-west-1:123456789012:cluster/aex-dev-export".to_owned(),
            task_definition:
                "arn:aws:ecs:eu-west-1:123456789012:task-definition/observation-export-task:7"
                    .to_owned(),
            container: "observation-export-task".to_owned(),
            subnets: vec!["subnet-0a1b2c3d".to_owned()],
            security_groups: vec!["sg-0123abcd".to_owned()],
            shards: 2,
            max_concurrent: 8,
            lease: std::time::Duration::from_mins(2),
        }
    }

    /// A hand-written fake of the `EXPORT#`/`CTRL#` rows.
    ///
    /// Every fixture item lives in control shard `0`, which is what a real
    /// sharded partition looks like from one shard's point of view: the other
    /// shards hold other exports, never the same ones.
    #[derive(Default)]
    struct FakeRows {
        due: Vec<DueItem>,
        due_fails: bool,
        claim_wins: bool,
        record: RecordAnswer,
        scanned_shards: Mutex<Vec<u8>>,
        claims: Mutex<Vec<String>>,
        recorded: Mutex<Vec<(String, u64, String)>>,
    }

    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    enum RecordAnswer {
        #[default]
        Recorded,
        Superseded,
    }

    impl FakeRows {
        fn winning(due: Vec<DueItem>) -> Self {
            Self {
                due,
                claim_wins: true,
                ..Self::default()
            }
        }
    }

    #[async_trait::async_trait]
    impl ExportRows for FakeRows {
        async fn due(
            &self,
            shard: u8,
            _now: Timestamp,
            limit: usize,
        ) -> Result<Vec<DueItem>, LauncherError> {
            self.scanned_shards
                .lock()
                .expect("the shard log is not poisoned")
                .push(shard);
            if self.due_fails {
                return Err(LauncherError::Rows {
                    operation: "due scan",
                    reason: "the index is not readable".to_owned(),
                });
            }
            if shard != 0 {
                return Ok(Vec::new());
            }
            Ok(self.due.iter().take(limit).cloned().collect())
        }

        async fn claim(
            &self,
            item: &DueItem,
            lease: &Lease,
        ) -> Result<ClaimOutcome, LauncherError> {
            assert!(lease.expires_at.unix_millis() > lease.now.unix_millis());
            assert!(!lease.owner.is_empty());
            self.claims
                .lock()
                .expect("the claim log is not poisoned")
                .push(item.state_pk());
            if self.claim_wins {
                Ok(ClaimOutcome::Won(Claim { fence: 4 }))
            } else {
                Ok(ClaimOutcome::Lost)
            }
        }

        async fn record_task(
            &self,
            item: &DueItem,
            claim: Claim,
            task_arn: &str,
        ) -> Result<RecordOutcome, LauncherError> {
            self.recorded
                .lock()
                .expect("the record log is not poisoned")
                .push((item.state_pk(), claim.fence, task_arn.to_owned()));
            match self.record {
                RecordAnswer::Recorded => Ok(RecordOutcome::Recorded),
                RecordAnswer::Superseded => Ok(RecordOutcome::Superseded),
            }
        }
    }

    /// A hand-written fake of the `ECS` port.
    struct FakeTasks {
        outcome: RunTaskOutcome,
        running: Vec<String>,
        stopped: Vec<String>,
        list_fails: bool,
        run_calls: Mutex<Vec<String>>,
        listed: Mutex<Vec<TaskStatus>>,
    }

    impl FakeTasks {
        fn new(outcome: RunTaskOutcome) -> Self {
            Self {
                outcome,
                running: Vec::new(),
                stopped: Vec::new(),
                list_fails: false,
                run_calls: Mutex::new(Vec::new()),
                listed: Mutex::new(Vec::new()),
            }
        }

        fn run_calls(&self) -> Vec<String> {
            self.run_calls
                .lock()
                .expect("the call log is not poisoned")
                .clone()
        }

        fn listed(&self) -> Vec<TaskStatus> {
            self.listed
                .lock()
                .expect("the list log is not poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl TaskLauncher for FakeTasks {
        async fn run_task(
            &self,
            request: &LaunchRequest<'_>,
        ) -> Result<RunTaskOutcome, LauncherError> {
            assert!(request.cluster.contains(":cluster/"));
            assert!(request.task_definition.contains(":task-definition/"));
            assert_eq!(request.container, "observation-export-task");
            assert_eq!(request.subnets, ["subnet-0a1b2c3d".to_owned()]);
            assert_eq!(request.security_groups, ["sg-0123abcd".to_owned()]);
            let overrides = request.overrides();
            assert_eq!(overrides[0].0, EXPORT_ID_ENV);
            assert_eq!(overrides[1].0, WORKSPACE_ID_ENV);
            self.run_calls
                .lock()
                .expect("the call log is not poisoned")
                .push(request.identity());
            Ok(self.outcome.clone())
        }

        async fn tasks_started_by(
            &self,
            cluster: &str,
            started_by: &str,
            status: TaskStatus,
        ) -> Result<Vec<String>, LauncherError> {
            assert!(cluster.contains(":cluster/"));
            assert!(started_by.starts_with("exp_"));
            self.listed
                .lock()
                .expect("the list log is not poisoned")
                .push(status);
            if self.list_fails {
                return Err(LauncherError::Tasks {
                    operation: "ListTasks",
                    reason: "the cluster is not describable".to_owned(),
                });
            }
            Ok(match status {
                TaskStatus::Running => self.running.clone(),
                TaskStatus::Stopped => self.stopped.clone(),
            })
        }
    }

    #[tokio::test]
    async fn a_named_task_is_recorded_without_any_reconciliation() {
        let rows = FakeRows::winning(vec![due(1)]);
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-1", now())
            .await
            .expect("the sweep completes");

        assert_eq!(report.claimed, 1);
        assert_eq!(report.launched, 1);
        assert_eq!(report.unresolved, 0);
        assert_eq!(launcher.tasks.run_calls().len(), 1);
        assert!(
            launcher.tasks.listed().is_empty(),
            "a named task needs no identity reconciliation"
        );
        let recorded = launcher
            .rows
            .recorded
            .lock()
            .expect("the record log is not poisoned")
            .clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1, 4, "the identity is written under the fence");
        assert!(recorded[0].2.contains(":task/"));
    }

    #[tokio::test]
    async fn a_lost_claim_skips_the_item_and_launches_nothing() {
        let rows = FakeRows {
            due: vec![due(1), due(2)],
            claim_wins: false,
            ..FakeRows::default()
        };
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-2", now())
            .await
            .expect("the sweep completes");

        assert_eq!(
            report.skipped, 2,
            "both due items were considered and skipped"
        );
        assert_eq!(report.scanned, 2);
        assert_eq!(report.claimed, 0);
        assert_eq!(report.launched, 0);
        assert!(
            launcher.tasks.run_calls().is_empty(),
            "a lost claim must never reach the cluster"
        );
        assert!(
            launcher
                .rows
                .recorded
                .lock()
                .expect("the record log is not poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn launch_never_duplicates_on_an_ambiguous_outcome() {
        // The task did start; `RunTask` simply did not say so. Resolving it by
        // identity is the difference between one export artifact and two.
        let arn = "arn:aws:ecs:eu-west-1:123456789012:task/aex/started".to_owned();
        let rows = FakeRows::winning(vec![due(3)]);
        let tasks = FakeTasks {
            stopped: vec![arn.clone()],
            ..FakeTasks::new(RunTaskOutcome::Unknown {
                reason: "the response named no task".to_owned(),
            })
        };
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-3", now())
            .await
            .expect("the sweep completes");

        assert_eq!(
            launcher.tasks.run_calls().len(),
            1,
            "an ambiguous outcome must never produce a second RunTask"
        );
        assert_eq!(
            launcher.tasks.listed(),
            RECONCILED_STATUSES.to_vec(),
            "the reconciliation covers RUNNING and STOPPED, in that order"
        );
        assert_eq!(report.launched, 1);
        assert_eq!(report.unresolved, 0);
        let recorded = launcher
            .rows
            .recorded
            .lock()
            .expect("the record log is not poisoned")
            .clone();
        assert_eq!(recorded[0].2, arn);
    }

    #[tokio::test]
    async fn a_running_task_short_circuits_the_reconciliation() {
        let rows = FakeRows::winning(vec![due(4)]);
        let tasks = FakeTasks {
            running: vec!["arn:aws:ecs:eu-west-1:123456789012:task/aex/live".to_owned()],
            ..FakeTasks::new(RunTaskOutcome::Refused {
                reason: "RESOURCE:MEMORY".to_owned(),
            })
        };
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-4", now())
            .await
            .expect("the sweep completes");

        assert_eq!(launcher.tasks.run_calls().len(), 1);
        assert_eq!(launcher.tasks.listed(), vec![TaskStatus::Running]);
        assert_eq!(report.launched, 1);
    }

    #[tokio::test]
    async fn an_unattributable_launch_is_left_for_the_next_due_window() {
        let rows = FakeRows::winning(vec![due(5)]);
        let tasks = FakeTasks::new(RunTaskOutcome::Unknown {
            reason: "the call timed out".to_owned(),
        });
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-5", now())
            .await
            .expect("the sweep completes");

        assert_eq!(
            launcher.tasks.run_calls().len(),
            1,
            "an unresolved outcome is still exactly one RunTask"
        );
        assert_eq!(launcher.tasks.listed(), RECONCILED_STATUSES.to_vec());
        assert_eq!(report.unresolved, 1);
        assert_eq!(report.launched, 0);
        assert!(
            launcher
                .rows
                .recorded
                .lock()
                .expect("the record log is not poisoned")
                .is_empty(),
            "nothing is recorded for a task nobody can name"
        );
    }

    #[tokio::test]
    async fn a_superseded_record_is_counted_and_never_relaunched() {
        let rows = FakeRows {
            record: RecordAnswer::Superseded,
            ..FakeRows::winning(vec![due(6)])
        };
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let launcher = Launcher::new(rows, tasks, settings());
        let report = launcher
            .sweep("request-6", now())
            .await
            .expect("the sweep completes");

        assert_eq!(report.superseded, 1);
        assert_eq!(report.launched, 0);
        assert_eq!(launcher.tasks.run_calls().len(), 1);
    }

    #[tokio::test]
    async fn a_reconciliation_failure_stops_the_sweep_rather_than_relaunching() {
        let rows = FakeRows::winning(vec![due(7)]);
        let tasks = FakeTasks {
            list_fails: true,
            ..FakeTasks::new(RunTaskOutcome::Unknown {
                reason: "the response named no task".to_owned(),
            })
        };
        let launcher = Launcher::new(rows, tasks, settings());
        let error = launcher
            .sweep("request-7", now())
            .await
            .expect_err("an unreconcilable launch fails the sweep");

        assert!(
            matches!(error, LauncherError::Tasks { .. }),
            "{error:?}: a reconciliation that cannot run must never fall back to a relaunch"
        );
        assert_eq!(launcher.tasks.run_calls().len(), 1);
    }

    #[tokio::test]
    async fn a_due_scan_failure_stops_the_sweep_before_any_claim() {
        let rows = FakeRows {
            due_fails: true,
            ..FakeRows::winning(vec![due(8)])
        };
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let launcher = Launcher::new(rows, tasks, settings());
        let error = launcher
            .sweep("request-8", now())
            .await
            .expect_err("an unreadable index fails the sweep");

        assert!(matches!(error, LauncherError::Rows { .. }), "{error:?}");
        assert!(launcher.tasks.run_calls().is_empty());
        assert!(
            launcher
                .rows
                .claims
                .lock()
                .expect("the claim log is not poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_sweep_never_exceeds_the_configured_concurrency() {
        let items: Vec<DueItem> = (1..=10).map(due).collect();
        let rows = FakeRows::winning(items);
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let bounded = LaunchSettings {
            max_concurrent: 3,
            shards: 4,
            ..settings()
        };
        let launcher = Launcher::new(rows, tasks, bounded);
        let report = launcher
            .sweep("request-9", now())
            .await
            .expect("the sweep completes");

        assert_eq!(report.scanned, 3);
        assert_eq!(report.launched, 3);
        assert_eq!(launcher.tasks.run_calls().len(), 3);
        assert_eq!(
            launcher
                .rows
                .scanned_shards
                .lock()
                .expect("the shard log is not poisoned")
                .as_slice(),
            [0],
            "the budget was exhausted on the first shard, so no further shard is read"
        );
    }

    #[tokio::test]
    async fn every_configured_shard_is_scanned_when_the_budget_allows() {
        let rows = FakeRows::winning(Vec::new());
        let tasks = FakeTasks::new(RunTaskOutcome::Started {
            task_arn: "arn:aws:ecs:eu-west-1:123456789012:task/aex/abc".to_owned(),
        });
        let wide = LaunchSettings {
            shards: 5,
            ..settings()
        };
        let launcher = Launcher::new(rows, tasks, wide);
        let report = launcher
            .sweep("request-10", now())
            .await
            .expect("the sweep completes");

        assert_eq!(report, super::SweepReport::default());
        assert_eq!(
            launcher
                .rows
                .scanned_shards
                .lock()
                .expect("the shard log is not poisoned")
                .as_slice(),
            [0, 1, 2, 3, 4],
            "a shard that is never read is a due export nobody ever launches"
        );
    }

    #[test]
    fn the_overrides_carry_exactly_the_export_and_workspace_identifiers() {
        let subnets = vec!["subnet-0a1b2c3d".to_owned()];
        let groups = vec!["sg-0123abcd".to_owned()];
        let request = LaunchRequest {
            cluster: "arn:aws:ecs:eu-west-1:123456789012:cluster/aex-dev-export",
            task_definition: "arn:aws:ecs:eu-west-1:123456789012:task-definition/t:1",
            container: "t",
            subnets: &subnets,
            security_groups: &groups,
            workspace: workspace(),
            export: export(1),
        };
        let overrides = request.overrides();
        assert_eq!(overrides.len(), 2);
        assert_eq!(overrides[0], (EXPORT_ID_ENV, export(1).to_string()));
        assert_eq!(overrides[1], (WORKSPACE_ID_ENV, workspace().to_string()));
        assert_eq!(request.identity(), export(1).to_string());
    }

    #[test]
    fn the_durable_spellings_are_the_ones_the_api_writes() {
        assert_eq!(STATE_ADMITTED, "admitted");
        assert_eq!(STATE_LAUNCHING, "launching");
        assert_eq!(TaskStatus::Running.as_str(), "RUNNING");
        assert_eq!(TaskStatus::Stopped.as_str(), "STOPPED");
        assert_eq!(shard_partition(3), "CTRL#export.launch#03");
    }

    #[test]
    fn the_due_window_ends_at_the_next_millisecond() {
        let bound = due_upper_bound(now()).expect("representable");
        let at_now = super::control_sk(now(), &export(1).to_string());
        assert!(at_now < bound, "{at_now} !< {bound}");
        let later = Timestamp::from_unix_millis(now().unix_millis() + 1).expect("representable");
        let after = super::control_sk(later, &export(1).to_string());
        assert!(bound < after, "{bound} !< {after}");
        assert_eq!(parse_control_sk(&at_now).expect("parses"), now());
    }

    #[test]
    fn a_key_outside_the_export_template_is_refused_rather_than_guessed() {
        let good = due(1).state_pk();
        let sort = due(1).state_sk();
        assert_eq!(
            parse_export_key(&good, &sort).expect("parses"),
            (workspace(), export(1))
        );
        let hostile: [(&str, &str); 5] = [
            ("OBS#W#wsp_x#logs", &sort),
            ("EXPORT#wsp_x", &sort),
            // The old two-component partition is not silently re-accepted.
            (&format!("EXPORT#{}#{}", workspace(), export(1)), &sort),
            (&good, "STATE"),
            (&good, "not-an-export"),
        ];
        for (pk, sk) in hostile {
            assert!(
                matches!(parse_export_key(pk, sk), Err(LauncherError::Key { .. })),
                "`{pk}`/`{sk}` was accepted"
            );
        }
    }

    #[test]
    fn a_lease_expires_strictly_after_the_instant_it_was_taken_at() {
        let lease =
            Lease::new("request", now(), std::time::Duration::from_secs(1)).expect("representable");
        assert_eq!(lease.owner, "request");
        assert_eq!(lease.now, now());
        assert_eq!(lease.expires_at.unix_millis(), now().unix_millis() + 1_000);
    }
}
