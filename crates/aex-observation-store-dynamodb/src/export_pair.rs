//! Atomic authority for a telemetry export and its canonical operation.
//!
//! The export row is the launch/task fence, while the operation row is the
//! customer-visible durable identity. Neither is useful alone. Mutable state is
//! therefore classified from one `TransactGetItems` snapshot, and every status
//! change coordinates the affected rows in one `TransactWriteItems`, with the
//! scope deletion row participating while a live scope is still required.

use std::collections::HashMap;
use std::time::Duration;

use aex_observation_app::ExportPlan;
use aex_observation_domain::keys::{self, ScopeKey};
use aex_operation_domain::{
    FailureClass, Operation, OperationFailure, OperationKind, OperationResult, OperationScope,
    OperationStatus, fail, revoke_telemetry_export, start, succeed,
};
use aex_session_dynamodb::attr::{Item, PK, SK, boolean, n, s};
use aex_session_dynamodb::codec::{decode_operation, encode_operation};
use aex_session_dynamodb::error::{
    Idempotence, Resolution, RetryPolicy, StoreError, classify, decode_cancellation_with_resolution,
};
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_session_dynamodb::wire_pending::StoredOperation;
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{ExportId, OperationId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::{ConditionCheckBuilder, PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{AttributeValue, Get, TransactGetItem};

const SCOPE_GUARD: Participant = Participant::new("export.scope_guard");
const OPERATION: Participant = Participant::new("export.operation");
const EXPORT: Participant = Participant::new("export.state");
const SCOPE_EXPORT: Participant = Participant::new("export.scope_directory");

/// A complete export/operation admission.
#[derive(Clone, Debug)]
pub struct ExportAdmissionCommit {
    /// The pure observation export plan.
    pub export: ExportPlan,
    /// The canonical queued operation admitted with it.
    pub operation: Operation,
    /// The exact deletion-fenced read scope.
    pub scope: ScopeKey,
    /// The deletion epoch observed before planning.
    pub deletion_epoch: u64,
}

/// How an admission resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportAdmissionOutcome {
    /// Both rows were inserted now.
    Inserted(Box<StoredOperation>),
    /// Both rows already held the same operation and intent.
    Replay(Box<StoredOperation>),
}

impl ExportAdmissionOutcome {
    /// The canonical operation returned to the caller in either case.
    #[must_use]
    pub fn operation(&self) -> &StoredOperation {
        match self {
            Self::Inserted(operation) | Self::Replay(operation) => operation,
        }
    }
}

/// Values the export task publishes after the artifact was verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPublish {
    /// The lease fence held by this task.
    pub fence: u64,
    /// The object key that was verified.
    pub object_key: Box<str>,
    /// The manifest digest.
    pub manifest_hash: Box<str>,
    /// Exact object length.
    pub object_bytes: u64,
    /// When the object became ready.
    pub ready_at: Timestamp,
    /// Canonical operation result for the same artifact.
    pub result: OperationResult,
}

/// The outcome of a worker settlement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportSettlement {
    /// The pair now carries the requested terminal state.
    Settled,
    /// A revoke, delete, or earlier terminal result won.
    Superseded {
        /// Stable explanation of the winning terminal state.
        reason: &'static str,
    },
}

/// How the task's generation-lease acceptance resolved.
#[derive(Clone, Debug)]
pub enum ExportStart {
    /// The export row after the paired acceptance transaction.
    Taken(Box<Item>),
    /// Another lease, revoke, deletion, or terminal state won.
    Lost {
        /// Stable explanation of the winning fence.
        reason: &'static str,
    },
}

struct StartAttempt {
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
    scope: ScopeKey,
    fence: u64,
    next_fence: u64,
    now: Timestamp,
}

/// A paired export transition failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportPairError {
    /// The caller reused its operation id for another request.
    #[error("the operation id already names a different export intent")]
    Conflict,
    /// No export exists at the requested workspace key.
    #[error("the export does not exist")]
    NotFound,
    /// The session/workspace deletion fence moved before the transaction.
    #[error("the export scope is no longer open at its pinned deletion epoch")]
    ScopeChanged,
    /// A generation lease must be live for a positive interval after acceptance.
    #[error("the export lease expiry must be later than its acceptance time")]
    InvalidLease,
    /// One row exists without its peer, or their immutable identities disagree.
    #[error("the export and operation authority rows disagree: {detail}")]
    Corrupt {
        /// Which paired invariant was violated.
        detail: String,
    },
    /// The regional store refused or could not resolve the transaction.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Cross-table authority over one export and its durable operation.
#[derive(Clone, Debug)]
pub struct ExportPairStore {
    dynamodb: aws_sdk_dynamodb::Client,
    observation_table: String,
    session_table: String,
}

impl ExportPairStore {
    /// Binds the two physical authority tables.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        observation_table: impl Into<String>,
        session_table: impl Into<String>,
    ) -> Self {
        Self {
            dynamodb,
            observation_table: observation_table.into(),
            session_table: session_table.into(),
        }
    }

    /// Atomically inserts the canonical operation and export state.
    ///
    /// An exact replay returns the original operation. A different intent under
    /// the same operation id conflicts, including after the scope was deleted.
    ///
    /// # Errors
    ///
    /// Returns an authority, validation, conflict, or store error when the pair
    /// cannot be admitted or resolved as an exact replay.
    pub async fn admit(
        &self,
        commit: &ExportAdmissionCommit,
    ) -> Result<ExportAdmissionOutcome, ExportPairError> {
        validate_admission(commit)?;
        let stored = StoredOperation {
            record: commit.operation.clone(),
            version: 1,
        };
        let operation_item = encode_operation(&stored).map_err(StoreError::from)?;
        let export_item = export_item(&commit.export);
        let mut plan = TransactionPlan::new(commit.operation.id.to_string());
        add_scope_guard(
            &mut plan,
            &self.observation_table,
            &self.session_table,
            commit.scope,
            commit.operation.workspace,
            commit.deletion_epoch,
        )?;
        plan.put(
            OPERATION,
            PutBuilder::default()
                .table_name(&self.session_table)
                .set_item(Some(operation_item))
                .condition_expression("attribute_not_exists(pk)"),
        )?;
        plan.put(
            EXPORT,
            PutBuilder::default()
                .table_name(&self.observation_table)
                .set_item(Some(export_item))
                .condition_expression("attribute_not_exists(pk)"),
        )?;
        plan.put(
            SCOPE_EXPORT,
            PutBuilder::default()
                .table_name(&self.observation_table)
                .set_item(Some(scope_export_item(commit)))
                .condition_expression("attribute_not_exists(pk)"),
        )?;

        match send(&self.dynamodb, &plan).await {
            Ok(()) => Ok(ExportAdmissionOutcome::Inserted(Box::new(stored))),
            Err(error) => match self.resolve_admission(commit).await? {
                Some(outcome) => Ok(outcome),
                None if matches!(error, StoreError::PreconditionFailed { participant, .. } if participant == SCOPE_GUARD) => {
                    Err(ExportPairError::ScopeChanged)
                }
                None => Err(ExportPairError::Store(error)),
            },
        }
    }

    /// Resolves an already-admitted identity without consulting volatile
    /// frontier, gap, or physical-plan state.
    ///
    /// The caller knows both deterministic keys, so the probe reads the pair in
    /// one serializable snapshot. An exact pair returns its canonical operation,
    /// an occupied operation identity with another intent conflicts, and no pair
    /// permits the caller to continue planning a fresh admission.
    ///
    /// # Errors
    ///
    /// Returns a conflict, corruption, decoding, or store error when the
    /// persisted pair cannot be resolved safely.
    pub async fn probe_admission(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        operation: OperationId,
        intent: IntentDigest,
        scope: ScopeKey,
    ) -> Result<Option<StoredOperation>, ExportPairError> {
        let (export_item, operation_item) = self
            .load_pair_snapshot(workspace, export, operation)
            .await?;
        if operation_item
            .as_ref()
            .is_some_and(|item| text(item, "workspaceId") != Some(workspace.to_string().as_str()))
        {
            return Err(ExportPairError::Conflict);
        }
        let stored = operation_item
            .as_ref()
            .map(|item| decode_operation(item, workspace))
            .transpose()
            .map_err(StoreError::from)?;
        match (export_item, stored) {
            (None, None) => Ok(None),
            (None, Some(operation)) => {
                if operation.record.kind == OperationKind::TelemetryExport
                    && operation.record.intent == intent
                    && operation.record.scope
                        == match scope {
                            ScopeKey::Session { session, .. } => OperationScope::Session(session),
                            ScopeKey::Workspace(workspace) => OperationScope::Workspace(workspace),
                        }
                {
                    Err(corrupt(
                        "canonical telemetry export operation has no export row",
                    ))
                } else {
                    Err(ExportPairError::Conflict)
                }
            }
            (Some(_), None) => Err(corrupt("export row has no canonical operation row")),
            (Some(export_item), Some(operation)) => {
                if operation.record.kind != OperationKind::TelemetryExport
                    || operation.record.intent != intent
                    || !operation_scope_matches(scope, &operation.record)
                    || text(&export_item, "intentDigest") != Some(intent.to_string().as_str())
                {
                    return Err(ExportPairError::Conflict);
                }
                ensure_pair(&export_item, &operation, scope)?;
                ensure_lifecycle_pair(&export_item, &operation)?;
                Ok(Some(operation))
            }
        }
    }

    /// Accepts the generation lease and moves the canonical operation from
    /// queued to running in the same transaction.
    ///
    /// # Errors
    ///
    /// Returns an invalid-lease, corruption, contention, or store error when
    /// the paired transition cannot be completed safely.
    pub async fn start(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        now: Timestamp,
        lease_expires_at: i64,
    ) -> Result<ExportStart, ExportPairError> {
        validate_lease_window(lease_expires_at, now.unix_millis())?;
        for _ in 0..3 {
            let (export_item, operation) = self.load_pair(workspace, export).await?;
            let scope = export_scope(&export_item)?;
            let operation_id = export_operation(&export_item)?;
            ensure_pair(&export_item, &operation, scope)?;
            if let Some(settlement) = paired_terminal(&export_item, &operation)? {
                let reason = match settlement {
                    ExportSettlement::Settled => "the export is already terminal",
                    ExportSettlement::Superseded { reason } => reason,
                };
                return Ok(ExportStart::Lost { reason });
            }
            let state =
                text(&export_item, "state").ok_or_else(|| corrupt("export row has no state"))?;
            match (state, operation.record.status) {
                ("admitted", OperationStatus::Queued) => {
                    return Ok(ExportStart::Lost {
                        reason: "the export has not crossed the launch fence",
                    });
                }
                ("launching", OperationStatus::Queued)
                | ("generating", OperationStatus::Running) => {}
                _ => return Err(corrupt("export lease and operation live states disagree")),
            }
            let fence =
                number(&export_item, "fence").ok_or_else(|| corrupt("export row has no fence"))?;
            let lease_expired = lease_is_expired(
                number_i64(&export_item, "leaseExpiresAt"),
                now.unix_millis(),
            );
            if !matches!(state, "launching" | "generating")
                || !lease_expired
                || flag(&export_item, "cancelRequested")
            {
                return Ok(ExportStart::Lost {
                    reason: "the export row is not leasable by this task",
                });
            }
            let next_fence = fence
                .checked_add(1)
                .ok_or_else(|| corrupt("export fence overflowed"))?;
            let pinned = number(&export_item, "deletionEpochPinned")
                .ok_or_else(|| corrupt("export row has no deletionEpochPinned"))?;
            let mut plan = TransactionPlan::new(format!("start-{export}-{next_fence}"));
            add_scope_guard(
                &mut plan,
                &self.observation_table,
                &self.session_table,
                scope,
                workspace,
                pinned,
            )?;
            match (state, operation.record.status) {
                ("launching", OperationStatus::Queued) => {
                    let running = start(&operation.record, now)
                        .map_err(|error| corrupt(error.to_string()))?
                        .operation;
                    plan.put(
                        OPERATION,
                        replace_operation(&self.session_table, &operation, running)?,
                    )?;
                }
                ("generating", OperationStatus::Running) => {}
                _ => unreachable!("the live pair was validated above"),
            }
            plan.update(
                EXPORT,
                start_update(
                    &self.observation_table,
                    workspace,
                    export,
                    operation_id,
                    state,
                    fence,
                    now,
                    lease_expires_at,
                ),
            )?;
            if let Some(outcome) = self
                .resolve_start_attempt(
                    send(&self.dynamodb, &plan).await,
                    StartAttempt {
                        workspace,
                        export,
                        operation: operation_id,
                        scope,
                        fence,
                        next_fence,
                        now,
                    },
                )
                .await?
            {
                return Ok(outcome);
            }
        }
        Err(StoreError::Contended.into())
    }

    async fn resolve_start_attempt(
        &self,
        result: Result<(), StoreError>,
        attempt: StartAttempt,
    ) -> Result<Option<ExportStart>, ExportPairError> {
        if result.is_ok() {
            let (item, operation) = self
                .load_pair_known(attempt.workspace, attempt.export, attempt.operation)
                .await?;
            ensure_pair(&item, &operation, attempt.scope)?;
            return resolve_start_replay(&item, &operation, attempt.next_fence)?
                .map(Some)
                .ok_or_else(|| corrupt("started pair did not carry the committed fence"));
        }

        let error = result.expect_err("the successful result returned above");
        if matches!(
            error,
            StoreError::PreconditionFailed { participant, .. } if participant == SCOPE_GUARD
        ) {
            let _ = self
                .fail(
                    attempt.workspace,
                    attempt.export,
                    attempt.fence,
                    OperationFailure::bare(deletion_error(attempt.scope), FailureClass::Terminal),
                    attempt.now,
                )
                .await?;
            return Ok(Some(ExportStart::Lost {
                reason: "the export scope was deleted before generation",
            }));
        }

        let (item, operation) = self
            .load_pair_known(attempt.workspace, attempt.export, attempt.operation)
            .await?;
        ensure_pair(&item, &operation, attempt.scope)?;
        if let Some(outcome) = resolve_start_replay(&item, &operation, attempt.next_fence)? {
            return Ok(Some(outcome));
        }
        if retryable_transition(&error) {
            return Ok(None);
        }
        Err(error.into())
    }

    /// Revokes the export and cancels its queued or running operation in the
    /// same transaction.
    ///
    /// # Errors
    ///
    /// Returns a scope, corruption, contention, or store error when the paired
    /// revocation cannot be completed safely.
    pub async fn revoke(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        expected_scope: ScopeKey,
        now: Timestamp,
    ) -> Result<Item, ExportPairError> {
        for _ in 0..3 {
            let (export_item, operation) = self.load_pair(workspace, export).await?;
            ensure_scope(&export_item, expected_scope)?;
            let operation_id = export_operation(&export_item)?;
            ensure_pair(&export_item, &operation, expected_scope)?;
            if text(&export_item, "state") == Some("revoked") {
                if paired_terminal(&export_item, &operation)?.is_some() {
                    return Ok(export_item);
                }
                return Err(corrupt(
                    "a revoked export has no compatible terminal operation",
                ));
            }

            match (text(&export_item, "state"), operation.record.status) {
                (Some("admitted" | "launching"), OperationStatus::Queued)
                | (Some("generating"), OperationStatus::Running) => {}
                _ if operation.record.status.is_terminal() => {}
                _ => return Err(corrupt("revoke found a non-paired live state")),
            }

            let mut plan = TransactionPlan::new(format!("revoke-{export}"));
            let export_update = revoke_update(
                &self.observation_table,
                workspace,
                export,
                operation_id,
                number(&export_item, "fence").unwrap_or(0),
                now,
            );
            plan.update(EXPORT, export_update)?;
            if matches!(
                operation.record.status,
                OperationStatus::Queued | OperationStatus::Running
            ) {
                let cancelled = revoke_telemetry_export(&operation.record, now)
                    .map_err(|error| corrupt(error.to_string()))?
                    .operation;
                plan.put(
                    OPERATION,
                    replace_operation(&self.session_table, &operation, cancelled)?,
                )?;
            } else if !operation.record.status.is_terminal() {
                return Err(corrupt(
                    "a telemetry export operation reached an unsupported nonterminal status",
                ));
            }

            match send(&self.dynamodb, &plan).await {
                Ok(()) => {
                    let (item, operation) = self
                        .load_pair_known(workspace, export, operation_id)
                        .await?;
                    if paired_terminal(&item, &operation)?.is_none() {
                        return Err(corrupt("revoked pair did not become terminal"));
                    }
                    return Ok(item);
                }
                Err(error) if retryable_transition(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(StoreError::Contended.into())
    }

    /// Atomically publishes a ready export and succeeds its operation.
    ///
    /// # Errors
    ///
    /// Returns a corruption, contention, or store error when the paired
    /// publication cannot be completed safely.
    pub async fn publish(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        request: &ExportPublish,
    ) -> Result<ExportSettlement, ExportPairError> {
        for _ in 0..3 {
            let (export_item, operation) = self.load_pair(workspace, export).await?;
            let scope = export_scope(&export_item)?;
            let operation_id = export_operation(&export_item)?;
            ensure_pair(&export_item, &operation, scope)?;
            if let Some(settlement) = publish_terminal(&export_item, &operation, request)? {
                return Ok(settlement);
            }
            if text(&export_item, "state") != Some("generating")
                || operation.record.status != OperationStatus::Running
            {
                return Err(corrupt("ready publication found a non-paired live state"));
            }
            let terminal = succeed(&operation.record, request.result.clone(), request.ready_at)
                .map_err(|error| corrupt(error.to_string()))?
                .operation;
            let pinned = number(&export_item, "deletionEpochPinned")
                .ok_or_else(|| corrupt("export row has no deletionEpochPinned"))?;
            let mut plan = TransactionPlan::new(format!("ready-{export}-{}", request.fence));
            add_scope_guard(
                &mut plan,
                &self.observation_table,
                &self.session_table,
                scope,
                workspace,
                pinned,
            )?;
            plan.put(
                OPERATION,
                replace_operation(&self.session_table, &operation, terminal)?,
            )?;
            plan.update(
                EXPORT,
                publish_update(
                    &self.observation_table,
                    workspace,
                    export,
                    operation_id,
                    request,
                ),
            )?;
            match send(&self.dynamodb, &plan).await {
                Ok(()) => return Ok(ExportSettlement::Settled),
                Err(error) => match self.resolve_publish(workspace, export, request).await? {
                    Some(settlement) => return Ok(settlement),
                    None if matches!(error, StoreError::PreconditionFailed { participant, .. } if participant == SCOPE_GUARD) =>
                    {
                        return self
                            .fail(
                                workspace,
                                export,
                                request.fence,
                                OperationFailure::bare(
                                    deletion_error(scope),
                                    FailureClass::Terminal,
                                ),
                                request.ready_at,
                            )
                            .await;
                    }
                    None if retryable_transition(&error) => {}
                    None => return Err(error.into()),
                },
            }
        }
        Err(StoreError::Contended.into())
    }

    /// Atomically marks generation failed and fails the canonical operation.
    ///
    /// # Errors
    ///
    /// Returns a corruption, contention, or store error when the paired failure
    /// transition cannot be completed safely.
    pub async fn fail(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        fence: u64,
        failure: OperationFailure,
        now: Timestamp,
    ) -> Result<ExportSettlement, ExportPairError> {
        for _ in 0..3 {
            let (export_item, operation) = self.load_pair(workspace, export).await?;
            let scope = export_scope(&export_item)?;
            let operation_id = export_operation(&export_item)?;
            ensure_pair(&export_item, &operation, scope)?;
            if let Some(settlement) =
                failure_terminal(&export_item, &operation, fence, &failure, now)?
            {
                return Ok(settlement);
            }
            if !matches!(
                (text(&export_item, "state"), operation.record.status),
                (Some("launching"), OperationStatus::Queued)
                    | (Some("generating"), OperationStatus::Running)
            ) {
                return Err(corrupt("failure settlement found a non-paired live state"));
            }
            let terminal = fail(&operation.record, failure.clone(), now)
                .map_err(|error| corrupt(error.to_string()))?
                .operation;
            let plan = failure_plan(
                &self.observation_table,
                &self.session_table,
                workspace,
                export,
                operation_id,
                fence,
                failure.code,
                now,
                &operation,
                terminal,
            )?;
            match send(&self.dynamodb, &plan).await {
                Ok(()) => return Ok(ExportSettlement::Settled),
                Err(error) => match self
                    .resolve_failure(workspace, export, fence, &failure, now)
                    .await?
                {
                    Some(settlement) => return Ok(settlement),
                    None if retryable_transition(&error) => {}
                    None => return Err(error.into()),
                },
            }
        }
        Err(StoreError::Contended.into())
    }

    async fn resolve_admission(
        &self,
        commit: &ExportAdmissionCommit,
    ) -> Result<Option<ExportAdmissionOutcome>, ExportPairError> {
        self.probe_admission(
            commit.operation.workspace,
            commit.export.export,
            commit.operation.id,
            commit.operation.intent,
            commit.scope,
        )
        .await
        .map(|operation| {
            operation.map(|operation| ExportAdmissionOutcome::Replay(Box::new(operation)))
        })
    }

    async fn resolve_publish(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        request: &ExportPublish,
    ) -> Result<Option<ExportSettlement>, ExportPairError> {
        let (export_item, operation) = self.load_pair(workspace, export).await?;
        publish_terminal(&export_item, &operation, request)
    }

    async fn resolve_failure(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        fence: u64,
        failure: &OperationFailure,
        now: Timestamp,
    ) -> Result<Option<ExportSettlement>, ExportPairError> {
        let (export_item, operation) = self.load_pair(workspace, export).await?;
        failure_terminal(&export_item, &operation, fence, failure, now)
    }

    async fn load_pair(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
    ) -> Result<(Item, StoredOperation), ExportPairError> {
        // The seed read discovers only the immutable operation identity. No
        // status decision is made from it; the following transaction is the
        // serializable authority for both mutable rows.
        let seed = self
            .load_export(workspace, export)
            .await?
            .ok_or(ExportPairError::NotFound)?;
        let operation = export_operation(&seed)?;
        self.load_pair_known(workspace, export, operation).await
    }

    async fn load_pair_known(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        operation: OperationId,
    ) -> Result<(Item, StoredOperation), ExportPairError> {
        let (export_item, operation_item) = self
            .load_pair_snapshot(workspace, export, operation)
            .await?;
        let export_item = export_item.ok_or(ExportPairError::NotFound)?;
        let operation = operation_item
            .as_ref()
            .map(|item| decode_operation(item, workspace))
            .transpose()
            .map_err(StoreError::from)?
            .ok_or_else(|| corrupt("export row has no canonical operation row"))?;
        let scope = export_scope(&export_item)?;
        ensure_pair(&export_item, &operation, scope)?;
        Ok((export_item, operation))
    }

    async fn load_pair_snapshot(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
        operation: OperationId,
    ) -> Result<(Option<Item>, Option<Item>), ExportPairError> {
        let reads = pair_gets(
            &self.observation_table,
            &self.session_table,
            workspace,
            export,
            operation,
        )?;
        let mut attempt = 1;
        loop {
            match self
                .dynamodb
                .transact_get_items()
                .set_transact_items(Some(reads.to_vec()))
                .send()
                .await
            {
                Ok(output) => {
                    let responses = output.responses.unwrap_or_default();
                    if responses.len() != 2 {
                        return Err(corrupt(
                            "the atomic export pair read returned an unexpected response count",
                        ));
                    }
                    let mut responses = responses.into_iter();
                    let export_item = responses.next().and_then(|response| response.item);
                    let operation_item = responses.next().and_then(|response| response.item);
                    return Ok((export_item, operation_item));
                }
                Err(error) => {
                    let classified = classify(&error, Idempotence::Read);
                    let Some(delay) = pair_snapshot_retry_delay(&classified, attempt) else {
                        return Err(classified.into());
                    };
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn load_export(
        &self,
        workspace: WorkspaceId,
        export: ExportId,
    ) -> Result<Option<Item>, ExportPairError> {
        self.get(
            &self.observation_table,
            &keys::export_pk(workspace),
            &keys::export_sk(export),
        )
        .await
    }

    async fn get(&self, table: &str, pk: &str, sk: &str) -> Result<Option<Item>, ExportPairError> {
        self.dynamodb
            .get_item()
            .table_name(table)
            .key(PK, s(pk))
            .key(SK, s(sk))
            .consistent_read(true)
            .send()
            .await
            .map(|output| output.item)
            .map_err(|error| classify(&error, Idempotence::Read).into())
    }
}

fn pair_gets(
    observation_table: &str,
    session_table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
) -> Result<[TransactGetItem; 2], StoreError> {
    let operation_key = aex_session_dynamodb::keys::operation(operation);
    let export_get = Get::builder()
        .table_name(observation_table)
        .set_key(Some(key(
            &keys::export_pk(workspace),
            &keys::export_sk(export),
        )))
        .build()
        .map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
    let operation_get = Get::builder()
        .table_name(session_table)
        .set_key(Some(key(&operation_key.pk, &operation_key.sk)))
        .build()
        .map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
    Ok([
        TransactGetItem::builder().get(export_get).build(),
        TransactGetItem::builder().get(operation_get).build(),
    ])
}

fn validate_admission(commit: &ExportAdmissionCommit) -> Result<(), ExportPairError> {
    if commit.operation.id.to_string() != commit.export.export_row.text["operationId"]
        || commit.operation.workspace.to_string() != commit.export.export_row.text["workspaceId"]
        || commit.scope.to_key() != commit.export.export_row.text["scopeKey"]
        || commit.deletion_epoch != commit.export.export_row.numbers["deletionEpochPinned"]
        || commit.operation.kind != OperationKind::TelemetryExport
        || commit.operation.status != OperationStatus::Queued
        || !operation_scope_matches(commit.scope, &commit.operation)
    {
        return Err(corrupt("admission plan identities are not the same pair"));
    }
    Ok(())
}

fn operation_scope_matches(scope: ScopeKey, operation: &Operation) -> bool {
    if scope.workspace() != operation.workspace {
        return false;
    }
    match scope {
        ScopeKey::Session { session, .. } => {
            operation.session == Some(session)
                && operation.scope == OperationScope::Session(session)
        }
        ScopeKey::Workspace(workspace) => {
            operation.session.is_none() && operation.scope == OperationScope::Workspace(workspace)
        }
    }
}

fn export_item(plan: &ExportPlan) -> Item {
    let row = &plan.export_row;
    let mut item = HashMap::from([(PK.to_owned(), s(&row.pk)), (SK.to_owned(), s(&row.sk))]);
    item.extend(
        row.text
            .iter()
            .map(|(name, value)| (name.clone(), s(value))),
    );
    item.extend(
        row.numbers
            .iter()
            .map(|(name, value)| (name.clone(), n(*value))),
    );
    item.extend(
        row.flags
            .iter()
            .map(|(name, value)| (name.clone(), boolean(*value))),
    );
    item.extend(row.lists.iter().map(|(name, values)| {
        (
            name.clone(),
            AttributeValue::L(values.iter().map(s).collect()),
        )
    }));
    item
}

/// The strongly-consistent scope directory entry paired with admission.
///
/// A GSI is intentionally not used for deletion proof: index propagation may
/// lag the transaction that admitted the export. This row commits beside the
/// export and canonical operation, so a scope enumerator can never observe a
/// complete admission without its exact export identity.
fn scope_export_item(commit: &ExportAdmissionCommit) -> Item {
    HashMap::from([
        (PK.to_owned(), s(keys::scope_export_pk(&commit.scope))),
        (
            SK.to_owned(),
            s(keys::scope_export_sk(commit.export.export)),
        ),
        ("itemType".to_owned(), s("scope_export")),
        ("scopeKey".to_owned(), s(commit.scope.to_key())),
        (
            "workspaceId".to_owned(),
            s(commit.operation.workspace.to_string()),
        ),
        ("exportId".to_owned(), s(commit.export.export.to_string())),
        ("operationId".to_owned(), s(commit.operation.id.to_string())),
        (
            "format".to_owned(),
            s(commit.export.export_row.text["format"].clone()),
        ),
    ])
}

fn add_scope_guard(
    plan: &mut TransactionPlan,
    observation_table: &str,
    session_table: &str,
    scope: ScopeKey,
    workspace: WorkspaceId,
    epoch: u64,
) -> Result<(), StoreError> {
    let builder = match scope {
        ScopeKey::Session { session, .. } => {
            let (pk, sk) = aex_session_dynamodb::stream_keys::head(session);
            ConditionCheckBuilder::default()
                .table_name(session_table)
                .set_key(Some(key(&pk, sk)))
                .condition_expression(
                    "#item = :head AND #workspace = :workspace AND #lifecycle = :active AND #epoch = :epoch",
                )
                .expression_attribute_names("#item", "itemType")
                .expression_attribute_names("#workspace", "workspaceId")
                .expression_attribute_names("#lifecycle", "lifecycle")
                .expression_attribute_names("#epoch", "deletionEpoch")
                .expression_attribute_values(":head", s("session_head"))
                .expression_attribute_values(":workspace", s(workspace.to_string()))
                .expression_attribute_values(":active", s("active"))
                .expression_attribute_values(":epoch", n(epoch))
        }
        ScopeKey::Workspace(_) => ConditionCheckBuilder::default()
            .table_name(observation_table)
            .set_key(Some(key(&keys::frontier_pk(&scope), keys::DELETION_SK)))
            .condition_expression(
                "attribute_not_exists(#pk) OR (#epoch = :epoch AND #state = :open)",
            )
            .expression_attribute_names("#pk", PK)
            .expression_attribute_names("#epoch", "deletionEpoch")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":epoch", n(epoch))
            .expression_attribute_values(":open", s("none")),
    };
    plan.condition_check(SCOPE_GUARD, builder)?;
    Ok(())
}

fn replace_operation(
    table: &str,
    current: &StoredOperation,
    next: Operation,
) -> Result<PutBuilder, StoreError> {
    let version = current
        .version
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid {
            detail: "an operation version cannot advance past u64::MAX".to_owned(),
        })?;
    let item = encode_operation(&StoredOperation {
        record: next,
        version,
    })?;
    Ok(PutBuilder::default()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression("#version = :version AND #intent = :intent AND #status = :status")
        .expression_attribute_names("#version", "version")
        .expression_attribute_names("#intent", "intentHash")
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":version", n(current.version))
        .expression_attribute_values(":intent", s(current.record.intent.to_string()))
        .expression_attribute_values(":status", s(current.record.status.as_str())))
}

fn revoke_update(
    table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
    fence: u64,
    now: Timestamp,
) -> UpdateBuilder {
    UpdateBuilder::default()
        .table_name(table)
        .set_key(Some(key(&keys::export_pk(workspace), &keys::export_sk(export))))
        .update_expression(
            "SET #state = :revoked, #revoked = :now, #cancel = :yes REMOVE #cPk, #cSk",
        )
        .condition_expression(
            "attribute_exists(pk) AND #operation = :operation AND #fence = :fence AND #state <> :revoked",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_names("#revoked", "revokedAt")
        .expression_attribute_names("#cancel", "cancelRequested")
        .expression_attribute_names("#cPk", "cPk")
        .expression_attribute_names("#cSk", "cSk")
        .expression_attribute_names("#operation", "operationId")
        .expression_attribute_names("#fence", "fence")
        .expression_attribute_values(":revoked", s("revoked"))
        .expression_attribute_values(":now", s(now.to_wire()))
        .expression_attribute_values(":yes", boolean(true))
        .expression_attribute_values(":operation", s(operation.to_string()))
        .expression_attribute_values(":fence", n(fence))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the lease update binds every observed fence component explicitly"
)]
fn start_update(
    table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
    state: &str,
    fence: u64,
    now: Timestamp,
    lease_expires_at: i64,
) -> UpdateBuilder {
    UpdateBuilder::default()
        .table_name(table)
        .set_key(Some(key(&keys::export_pk(workspace), &keys::export_sk(export))))
        .update_expression("SET #state = :generating, #fence = :next, #lease = :until")
        .condition_expression(
            "#state = :state AND #fence = :fence AND #operation = :operation AND #cancel = :no AND (attribute_not_exists(#lease) OR #lease <= :now)",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_names("#fence", "fence")
        .expression_attribute_names("#lease", "leaseExpiresAt")
        .expression_attribute_names("#operation", "operationId")
        .expression_attribute_names("#cancel", "cancelRequested")
        .expression_attribute_values(":generating", s("generating"))
        .expression_attribute_values(":state", s(state))
        .expression_attribute_values(":fence", n(fence))
        .expression_attribute_values(":next", n(fence.saturating_add(1)))
        .expression_attribute_values(":operation", s(operation.to_string()))
        .expression_attribute_values(":no", boolean(false))
        .expression_attribute_values(":now", AttributeValue::N(now.unix_millis().to_string()))
        .expression_attribute_values(
            ":until",
            AttributeValue::N(lease_expires_at.to_string()),
        )
}

fn publish_update(
    table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
    request: &ExportPublish,
) -> UpdateBuilder {
    UpdateBuilder::default()
        .table_name(table)
        .set_key(Some(key(&keys::export_pk(workspace), &keys::export_sk(export))))
        .update_expression(
            "SET #state = :ready, #object = :object, #manifest = :manifest, #bytes = :bytes, #readyAt = :now REMOVE #cPk, #cSk",
        )
        .condition_expression(
            "#state = :generating AND #fence = :fence AND #operation = :operation AND #cancel = :no",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_names("#object", "objectKey")
        .expression_attribute_names("#manifest", "manifestHash")
        .expression_attribute_names("#bytes", "objectBytes")
        .expression_attribute_names("#readyAt", "readyAt")
        .expression_attribute_names("#cPk", "cPk")
        .expression_attribute_names("#cSk", "cSk")
        .expression_attribute_names("#fence", "fence")
        .expression_attribute_names("#operation", "operationId")
        .expression_attribute_names("#cancel", "cancelRequested")
        .expression_attribute_values(":ready", s("ready"))
        .expression_attribute_values(":generating", s("generating"))
        .expression_attribute_values(":object", s(request.object_key.as_ref()))
        .expression_attribute_values(":manifest", s(request.manifest_hash.as_ref()))
        .expression_attribute_values(":bytes", n(request.object_bytes))
        .expression_attribute_values(":now", s(request.ready_at.to_wire()))
        .expression_attribute_values(":fence", n(request.fence))
        .expression_attribute_values(":operation", s(operation.to_string()))
        .expression_attribute_values(":no", boolean(false))
}

fn failure_update(
    table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation: OperationId,
    fence: u64,
    code: ErrorCode,
    now: Timestamp,
) -> UpdateBuilder {
    UpdateBuilder::default()
        .table_name(table)
        .set_key(Some(key(&keys::export_pk(workspace), &keys::export_sk(export))))
        .update_expression(
            "SET #state = :failed, #failedAt = :now, #failure = :failure REMOVE #cPk, #cSk",
        )
        .condition_expression(
            "#state IN (:launching, :generating) AND #fence = :fence AND #operation = :operation AND #cancel = :no",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_names("#failedAt", "failedAt")
        .expression_attribute_names("#failure", "failureCode")
        .expression_attribute_names("#cPk", "cPk")
        .expression_attribute_names("#cSk", "cSk")
        .expression_attribute_names("#fence", "fence")
        .expression_attribute_names("#operation", "operationId")
        .expression_attribute_names("#cancel", "cancelRequested")
        .expression_attribute_values(":failed", s("failed"))
        .expression_attribute_values(":launching", s("launching"))
        .expression_attribute_values(":generating", s("generating"))
        .expression_attribute_values(":now", s(now.to_wire()))
        .expression_attribute_values(":failure", s(code.as_str()))
        .expression_attribute_values(":fence", n(fence))
        .expression_attribute_values(":operation", s(operation.to_string()))
        .expression_attribute_values(":no", boolean(false))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the paired failure plan binds both observed identities and their fence explicitly"
)]
fn failure_plan(
    observation_table: &str,
    session_table: &str,
    workspace: WorkspaceId,
    export: ExportId,
    operation_id: OperationId,
    fence: u64,
    code: ErrorCode,
    now: Timestamp,
    operation: &StoredOperation,
    terminal: Operation,
) -> Result<TransactionPlan, StoreError> {
    let mut plan = TransactionPlan::new(format!("failed-{export}-{fence}"));
    plan.put(
        OPERATION,
        replace_operation(session_table, operation, terminal)?,
    )?;
    plan.update(
        EXPORT,
        failure_update(
            observation_table,
            workspace,
            export,
            operation_id,
            fence,
            code,
            now,
        ),
    )?;
    Ok(plan)
}

fn resolve_start_replay(
    export: &Item,
    operation: &StoredOperation,
    expected_fence: u64,
) -> Result<Option<ExportStart>, ExportPairError> {
    if let Some(settlement) = paired_terminal(export, operation)? {
        let reason = match settlement {
            ExportSettlement::Settled => "the export is already terminal",
            ExportSettlement::Superseded { reason } => reason,
        };
        return Ok(Some(ExportStart::Lost { reason }));
    }
    if text(export, "state") == Some("generating")
        && number(export, "fence") == Some(expected_fence)
        && operation.record.status == OperationStatus::Running
    {
        return Ok(Some(ExportStart::Taken(Box::new(export.clone()))));
    }
    Ok(None)
}

fn paired_terminal(
    export: &Item,
    operation: &StoredOperation,
) -> Result<Option<ExportSettlement>, ExportPairError> {
    Ok(match (text(export, "state"), operation.record.status) {
        (Some("ready"), OperationStatus::Succeeded) | (Some("failed"), OperationStatus::Failed) => {
            Some(ExportSettlement::Settled)
        }
        (Some("revoked"), OperationStatus::Cancelled) => Some(ExportSettlement::Superseded {
            reason: "the export was revoked",
        }),
        (Some("revoked"), OperationStatus::Succeeded | OperationStatus::Failed) => {
            Some(ExportSettlement::Superseded {
                reason: "the terminal export was revoked",
            })
        }
        (Some("expired"), OperationStatus::Succeeded) => Some(ExportSettlement::Superseded {
            reason: "the export expired",
        }),
        (_, status) if status.is_terminal() => {
            return Err(corrupt(
                "export and canonical operation terminal states disagree",
            ));
        }
        _ => None,
    })
}

fn ensure_lifecycle_pair(
    export: &Item,
    operation: &StoredOperation,
) -> Result<(), ExportPairError> {
    if paired_terminal(export, operation)?.is_some() {
        return Ok(());
    }
    if matches!(
        (text(export, "state"), operation.record.status),
        (Some("admitted" | "launching"), OperationStatus::Queued)
            | (Some("generating"), OperationStatus::Running)
    ) {
        Ok(())
    } else {
        Err(corrupt(
            "export and canonical operation live states disagree",
        ))
    }
}

fn publish_terminal(
    export: &Item,
    operation: &StoredOperation,
    request: &ExportPublish,
) -> Result<Option<ExportSettlement>, ExportPairError> {
    let Some(_) = paired_terminal(export, operation)? else {
        return Ok(None);
    };
    if text(export, "state") != Some("ready")
        || operation.record.status != OperationStatus::Succeeded
    {
        return Ok(Some(ExportSettlement::Superseded {
            reason: "another terminal export transition won",
        }));
    }
    let exact = number(export, "fence") == Some(request.fence)
        && text(export, "objectKey") == Some(request.object_key.as_ref())
        && text(export, "manifestHash") == Some(request.manifest_hash.as_ref())
        && number(export, "objectBytes") == Some(request.object_bytes)
        && text(export, "readyAt") == Some(request.ready_at.to_wire().as_str())
        && operation.record.result.as_ref() == Some(&request.result);
    Ok(Some(if exact {
        ExportSettlement::Settled
    } else {
        ExportSettlement::Superseded {
            reason: "another export publication won",
        }
    }))
}

fn failure_terminal(
    export: &Item,
    operation: &StoredOperation,
    fence: u64,
    failure: &OperationFailure,
    now: Timestamp,
) -> Result<Option<ExportSettlement>, ExportPairError> {
    let Some(_) = paired_terminal(export, operation)? else {
        return Ok(None);
    };
    if text(export, "state") != Some("failed") || operation.record.status != OperationStatus::Failed
    {
        return Ok(Some(ExportSettlement::Superseded {
            reason: "another terminal export transition won",
        }));
    }
    let exact = number(export, "fence") == Some(fence)
        && text(export, "failureCode") == Some(failure.code.as_str())
        && text(export, "failedAt") == Some(now.to_wire().as_str())
        && operation.record.error.as_ref() == Some(failure);
    Ok(Some(if exact {
        ExportSettlement::Settled
    } else {
        ExportSettlement::Superseded {
            reason: "another export failure won",
        }
    }))
}

fn ensure_pair(
    export: &Item,
    operation: &StoredOperation,
    scope: ScopeKey,
) -> Result<(), ExportPairError> {
    if export_operation(export)? != operation.record.id
        || text(export, "workspaceId") != Some(operation.record.workspace.to_string().as_str())
        || text(export, "scopeKey") != Some(scope.to_key().as_str())
        || text(export, "intentDigest") != Some(operation.record.intent.to_string().as_str())
        || operation.record.kind != OperationKind::TelemetryExport
        || !operation_scope_matches(scope, &operation.record)
    {
        return Err(corrupt("immutable export/operation identities disagree"));
    }
    Ok(())
}

fn ensure_scope(item: &Item, expected: ScopeKey) -> Result<(), ExportPairError> {
    if text(item, "scopeKey") == Some(expected.to_key().as_str()) {
        Ok(())
    } else {
        Err(ExportPairError::NotFound)
    }
}

fn export_scope(item: &Item) -> Result<ScopeKey, ExportPairError> {
    text(item, "scopeKey")
        .and_then(|scope| ScopeKey::parse(scope).ok())
        .ok_or_else(|| corrupt("export row has no valid scopeKey"))
}

fn export_operation(item: &Item) -> Result<OperationId, ExportPairError> {
    text(item, "operationId")
        .and_then(|operation| operation.parse().ok())
        .ok_or_else(|| corrupt("export row has no valid operationId"))
}

fn deletion_error(scope: ScopeKey) -> ErrorCode {
    match scope {
        ScopeKey::Session { .. } => ErrorCode::SessionDeleted,
        ScopeKey::Workspace(_) => ErrorCode::NotFound,
    }
}

fn text<'a>(item: &'a Item, name: &str) -> Option<&'a str> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
}

fn number(item: &Item, name: &str) -> Option<u64> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
}

fn number_i64(item: &Item, name: &str) -> Option<i64> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
}

fn flag(item: &Item, name: &str) -> bool {
    item.get(name)
        .and_then(|value| value.as_bool().ok())
        .copied()
        .unwrap_or(false)
}

fn corrupt(detail: impl Into<String>) -> ExportPairError {
    ExportPairError::Corrupt {
        detail: detail.into(),
    }
}

fn retryable_transition(error: &StoreError) -> bool {
    error.retryable()
        || matches!(
            error,
            StoreError::PreconditionFailed { .. }
                | StoreError::CommitAmbiguous {
                    resolve_by: Resolution::TargetItem
                }
        )
}

fn pair_snapshot_retry_delay(error: &StoreError, attempt: u32) -> Option<Duration> {
    let policy = RetryPolicy::PINNED;
    if !error.retryable() || attempt >= policy.attempts {
        return None;
    }
    let policy_delay = policy.backoff(attempt.saturating_add(1));
    let delay = match error {
        StoreError::Throttled { retry_after } => policy_delay.max(*retry_after),
        _ => policy_delay,
    };
    Some(delay)
}

fn lease_is_expired(expires_at: Option<i64>, now: i64) -> bool {
    expires_at.is_none_or(|expires| expires <= now)
}

fn lease_window_is_valid(expires_at: i64, now: i64) -> bool {
    expires_at > now
}

fn validate_lease_window(expires_at: i64, now: i64) -> Result<(), ExportPairError> {
    lease_window_is_valid(expires_at, now)
        .then_some(())
        .ok_or(ExportPairError::InvalidLease)
}

async fn send(client: &aws_sdk_dynamodb::Client, plan: &TransactionPlan) -> Result<(), StoreError> {
    let participants = plan.participants().to_vec();
    let request = plan.compile(client)?;
    match request.send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let decoded = error.as_service_error().map_or_else(
                || classify(&error, Idempotence::Write(Resolution::TargetItem)),
                |service| {
                    decode_cancellation_with_resolution(
                        service,
                        &participants,
                        Resolution::TargetItem,
                    )
                },
            );
            Err(decoded)
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_app::{ExportAdmission, ExportScope};
    use aex_operation_domain::{
        FailureClass, Operation, OperationFailure, OperationKind, OperationResult, OperationScope,
        OperationStatus,
    };
    use aex_session_dynamodb::attr::{Item, PK, SK, n, s};
    use aex_session_dynamodb::error::{Resolution, StoreError};
    use aex_session_dynamodb::wire_pending::StoredOperation;
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{ExportId, OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        EXPORT, ExportPairError, ExportPublish, ExportSettlement, ExportStart, OPERATION,
        deletion_error, failure_plan, failure_terminal, lease_is_expired, lease_window_is_valid,
        pair_gets, pair_snapshot_retry_delay, paired_terminal, publish_terminal,
        resolve_start_replay, retryable_transition, scope_export_item,
    };

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn operation(status: OperationStatus) -> StoredOperation {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        StoredOperation {
            record: Operation {
                id: OperationId::from_uuid7(Uuid7::compose(2, [2; 10])),
                workspace,
                session: None,
                kind: OperationKind::TelemetryExport,
                status,
                intent: IntentDigest::from_bytes([3; 32]),
                scope: OperationScope::Workspace(workspace),
                progress: None,
                cursor: None,
                cancel_requested: status == OperationStatus::Cancelled,
                result: None,
                error: None,
                created_at: at(1),
                started_at: (status != OperationStatus::Queued).then(|| at(2)),
                updated_at: at(2),
                committed_at: None,
                terminal_at: status.is_terminal().then(|| at(2)),
            },
            version: 1,
        }
    }

    fn export(state: &str, operation: &StoredOperation) -> Item {
        Item::from([
            ("state".to_owned(), s(state)),
            ("operationId".to_owned(), s(operation.record.id.to_string())),
            (
                "workspaceId".to_owned(),
                s(operation.record.workspace.to_string()),
            ),
            (
                "scopeKey".to_owned(),
                s(
                    aex_observation_domain::keys::ScopeKey::Workspace(operation.record.workspace)
                        .to_key(),
                ),
            ),
            (
                "intentDigest".to_owned(),
                s(operation.record.intent.to_string()),
            ),
            ("fence".to_owned(), n(1)),
        ])
    }

    #[test]
    fn admission_directory_names_the_exact_session_export_and_operation() {
        let mut queued = operation(OperationStatus::Queued).record;
        let session = SessionId::from_uuid7(Uuid7::compose(5, [5; 10]));
        queued.session = Some(session);
        queued.scope = OperationScope::Session(session);
        queued.intent = IntentDigest::from_bytes([7; 32]);
        let scope = aex_observation_domain::keys::ScopeKey::Session {
            workspace: queued.workspace,
            session,
        };
        let export = aex_observation_app::export::plan(&ExportAdmission {
            operation: queued.id,
            workspace: queued.workspace,
            scope: ExportScope::Session(session),
            format: "ndjson".to_owned(),
            completeness: "allow_gaps".to_owned(),
            normalized_query: "{\"signal\":\"logs\"}".to_owned(),
            partitions: vec!["OBS#fixture".to_owned()],
            intent: [7; 32],
            deletion_epoch: 3,
            snapshot: at(10),
            gaps: Vec::new(),
            launch_at: at(10),
            now: at(10),
        })
        .expect("export plans");
        let export_id = export.export;
        let item = scope_export_item(&super::ExportAdmissionCommit {
            export,
            operation: queued.clone(),
            scope,
            deletion_epoch: 3,
        });

        assert_eq!(
            item[PK].as_s().expect("directory partition"),
            &aex_observation_domain::keys::scope_export_pk(&scope)
        );
        assert_eq!(
            item[SK].as_s().expect("directory key"),
            &aex_observation_domain::keys::scope_export_sk(export_id)
        );
        assert_eq!(
            item["operationId"].as_s().expect("operation"),
            &queued.id.to_string()
        );
        assert_eq!(item["scopeKey"].as_s().expect("scope"), &scope.to_key());
        assert_eq!(item["format"].as_s().expect("format"), "ndjson");
    }

    #[test]
    fn a_lease_is_half_open_and_equality_allows_takeover() {
        assert!(!lease_is_expired(Some(11), 10));
        assert!(lease_is_expired(Some(10), 10));
        assert!(lease_is_expired(None, 10));
    }

    #[test]
    fn a_new_lease_must_extend_past_its_acceptance_instant() {
        assert!(!lease_window_is_valid(9, 10));
        assert!(!lease_window_is_valid(10, 10));
        assert!(lease_window_is_valid(11, 10));
    }

    #[test]
    fn an_ambiguous_or_lost_condition_is_read_resolved_before_retry() {
        assert!(retryable_transition(&StoreError::CommitAmbiguous {
            resolve_by: Resolution::TargetItem,
        }));
        assert!(retryable_transition(&StoreError::PreconditionFailed {
            participant: EXPORT,
            observed: None,
        }));
        assert!(!retryable_transition(&StoreError::IdempotencyConflict));
    }

    #[test]
    fn an_atomic_pair_read_waits_twice_with_bounded_backoff() {
        let contention = (1..=aex_session_dynamodb::error::RetryPolicy::PINNED.attempts)
            .filter_map(|attempt| pair_snapshot_retry_delay(&StoreError::Contended, attempt))
            .collect::<Vec<_>>();
        assert_eq!(
            contention,
            vec![
                std::time::Duration::from_millis(50),
                std::time::Duration::from_millis(100)
            ],
            "three attempts have exactly two policy waits"
        );
        assert!(
            contention
                .iter()
                .all(|delay| *delay <= aex_session_dynamodb::error::RetryPolicy::PINNED.cap)
        );
        assert_eq!(
            pair_snapshot_retry_delay(
                &StoreError::Unavailable {
                    detail: "fixture".to_owned(),
                },
                1,
            ),
            contention.first().copied(),
            "unavailability takes the same first pinned delay"
        );

        let advertised = std::time::Duration::from_millis(275);
        assert_eq!(
            pair_snapshot_retry_delay(
                &StoreError::Throttled {
                    retry_after: advertised,
                },
                1,
            ),
            Some(advertised),
            "provider Retry-After is never shortened"
        );
        assert_eq!(
            pair_snapshot_retry_delay(
                &StoreError::Invalid {
                    detail: "fixture".to_owned(),
                },
                1,
            ),
            None
        );
    }

    #[test]
    fn idempotent_revoke_never_hides_a_queued_operation() {
        let queued = operation(OperationStatus::Queued);
        assert_eq!(
            paired_terminal(&export("revoked", &queued), &queued).expect("the pair is readable"),
            None,
            "the revoke path must reject this instead of replaying it"
        );
        let cancelled = operation(OperationStatus::Cancelled);
        assert!(
            paired_terminal(&export("revoked", &cancelled), &cancelled)
                .expect("the pair is compatible")
                .is_some()
        );
    }

    #[test]
    fn an_ambiguous_start_resolves_only_the_exact_running_fence() {
        let running = operation(OperationStatus::Running);
        let mut item = export("generating", &running);
        item.insert("fence".to_owned(), n(2));
        assert!(matches!(
            resolve_start_replay(&item, &running, 2).expect("the pair resolves"),
            Some(ExportStart::Taken(_))
        ));
        assert!(
            resolve_start_replay(&item, &running, 3)
                .expect("a different fence is readable")
                .is_none()
        );
    }

    #[test]
    fn a_concurrent_revoke_after_start_commit_resolves_as_lost() {
        let cancelled = operation(OperationStatus::Cancelled);
        assert!(matches!(
            resolve_start_replay(&export("revoked", &cancelled), &cancelled, 2)
                .expect("the terminal pair resolves"),
            Some(ExportStart::Lost { .. })
        ));
    }

    #[test]
    fn every_status_pair_snapshot_is_one_cross_table_transaction() {
        let operation = operation(OperationStatus::Queued);
        let export = ExportId::from_uuid7(Uuid7::compose(4, [4; 10]));
        let reads = pair_gets(
            "observation-authority",
            "session-authority",
            operation.record.workspace,
            export,
            operation.record.id,
        )
        .expect("the atomic pair read plans");
        let export_get = reads[0].get().expect("the export read");
        let operation_get = reads[1].get().expect("the operation read");
        assert_eq!(export_get.table_name(), "observation-authority");
        assert_eq!(operation_get.table_name(), "session-authority");
        assert_eq!(
            export_get.key()[PK].as_s().expect("the export partition"),
            &aex_observation_domain::keys::export_pk(operation.record.workspace)
        );
        assert_eq!(
            export_get.key()[SK].as_s().expect("the export sort key"),
            &aex_observation_domain::keys::export_sk(export)
        );
        let operation_key = aex_session_dynamodb::keys::operation(operation.record.id);
        assert_eq!(
            operation_get.key()[PK]
                .as_s()
                .expect("the operation partition"),
            &operation_key.pk
        );
        assert_eq!(
            operation_get.key()[SK]
                .as_s()
                .expect("the operation sort key"),
            &operation_key.sk
        );
    }

    #[test]
    fn publish_vs_revoke_classifies_the_coherent_revoked_snapshot_as_superseded() {
        let cancelled = operation(OperationStatus::Cancelled);
        let request = ExportPublish {
            fence: 1,
            object_key: "exports/object.ndjson".into(),
            manifest_hash: "blake3:loser".into(),
            object_bytes: 42,
            ready_at: at(30),
            result: OperationResult::receipt(),
        };
        assert!(matches!(
            publish_terminal(&export("revoked", &cancelled), &cancelled, &request)
                .expect("the atomic revoked pair resolves"),
            Some(ExportSettlement::Superseded { .. })
        ));
    }

    #[test]
    fn an_ambiguous_publish_settles_only_the_exact_terminal_payload_and_fence() {
        let request = ExportPublish {
            fence: 7,
            object_key: "exports/object.ndjson".into(),
            manifest_hash: "blake3:published".into(),
            object_bytes: 42,
            ready_at: at(30),
            result: OperationResult::receipt(),
        };
        let mut succeeded = operation(OperationStatus::Succeeded);
        succeeded.record.result = Some(request.result.clone());
        let mut item = export("ready", &succeeded);
        item.insert("fence".to_owned(), n(request.fence));
        item.insert("objectKey".to_owned(), s(request.object_key.as_ref()));
        item.insert("manifestHash".to_owned(), s(request.manifest_hash.as_ref()));
        item.insert("objectBytes".to_owned(), n(request.object_bytes));
        item.insert("readyAt".to_owned(), s(request.ready_at.to_wire()));

        assert_eq!(
            publish_terminal(&item, &succeeded, &request).expect("the exact replay resolves"),
            Some(ExportSettlement::Settled)
        );
        let mut other = request.clone();
        other.fence += 1;
        assert!(matches!(
            publish_terminal(&item, &succeeded, &other).expect("the winner resolves"),
            Some(ExportSettlement::Superseded { .. })
        ));
    }

    #[test]
    fn an_ambiguous_failure_settles_only_the_exact_terminal_error_and_fence() {
        let failure =
            OperationFailure::bare(ErrorCode::TelemetryIncomplete, FailureClass::Terminal);
        let mut failed = operation(OperationStatus::Failed);
        failed.record.error = Some(failure.clone());
        let mut item = export("failed", &failed);
        item.insert("fence".to_owned(), n(7));
        item.insert("failureCode".to_owned(), s(failure.code.as_str()));
        item.insert("failedAt".to_owned(), s(at(30).to_wire()));

        assert_eq!(
            failure_terminal(&item, &failed, 7, &failure, at(30))
                .expect("the exact replay resolves"),
            Some(ExportSettlement::Settled)
        );
        let other = OperationFailure::bare(ErrorCode::InternalError, FailureClass::Terminal);
        assert!(matches!(
            failure_terminal(&item, &failed, 7, &other, at(30)).expect("the winner resolves"),
            Some(ExportSettlement::Superseded { .. })
        ));
        assert!(matches!(
            failure_terminal(&item, &failed, 8, &failure, at(30)).expect("the winner resolves"),
            Some(ExportSettlement::Superseded { .. })
        ));
    }

    #[test]
    fn ambiguous_terminal_transitions_resolve_their_exact_pairs() {
        let cancelled = operation(OperationStatus::Cancelled);
        assert!(matches!(
            paired_terminal(&export("revoked", &cancelled), &cancelled)
                .expect("revoke replay resolves"),
            Some(super::ExportSettlement::Superseded { .. })
        ));

        let succeeded = operation(OperationStatus::Succeeded);
        assert_eq!(
            paired_terminal(&export("ready", &succeeded), &succeeded)
                .expect("publish replay resolves"),
            Some(super::ExportSettlement::Settled)
        );

        let failed = operation(OperationStatus::Failed);
        assert_eq!(
            paired_terminal(&export("failed", &failed), &failed).expect("failure replay resolves"),
            Some(super::ExportSettlement::Settled)
        );
    }

    #[test]
    fn scope_loss_failure_is_an_unguarded_atomic_pair_settlement() {
        let current = operation(OperationStatus::Running);
        let terminal = aex_operation_domain::fail(
            &current.record,
            aex_operation_domain::OperationFailure::bare(
                ErrorCode::SessionDeleted,
                aex_operation_domain::FailureClass::Terminal,
            ),
            at(3),
        )
        .expect("running exports may fail")
        .operation;
        let export = ExportId::from_uuid7(Uuid7::compose(4, [4; 10]));
        let plan = failure_plan(
            "observation-authority",
            "session-authority",
            current.record.workspace,
            export,
            current.record.id,
            1,
            ErrorCode::SessionDeleted,
            at(3),
            &current,
            terminal,
        )
        .expect("the settlement plans");
        assert_eq!(
            plan.participants(),
            &[OPERATION, EXPORT],
            "the lost deletion guard is deliberately absent while both pair rows remain atomic"
        );
        let session = SessionId::from_uuid7(Uuid7::compose(5, [5; 10]));
        assert_eq!(
            deletion_error(aex_observation_domain::keys::ScopeKey::Session {
                workspace: current.record.workspace,
                session,
            }),
            ErrorCode::SessionDeleted
        );
        assert_eq!(
            deletion_error(aex_observation_domain::keys::ScopeKey::Workspace(
                current.record.workspace
            )),
            ErrorCode::NotFound
        );
    }

    #[test]
    fn terminal_disagreement_is_corruption_not_a_retry() {
        let failed = operation(OperationStatus::Failed);
        assert!(matches!(
            paired_terminal(&export("ready", &failed), &failed),
            Err(ExportPairError::Corrupt { .. })
        ));
    }
}
