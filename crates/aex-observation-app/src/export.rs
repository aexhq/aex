//! The pure telemetry-export admission plan.
//!
//! Nothing here talks to a table, a clock or AWS. The plan is a value: the
//! export identity, every attribute the export row must carry, and the shape of
//! the durable transaction that must commit them. Keeping it pure is what lets
//! the identity, the replay rule and the row completeness be asserted exactly,
//! with no engine and no credentials — the same reason `aex-usage-app`'s fold is
//! a neutral transaction rather than a client call.
//!
//! Three rules the shape enforces rather than documents.
//!
//! **The export identity is derived, never minted.** It is a function of
//! `(workspace, operationId)` and nothing else, so a replayed `Aex-Operation-Id`
//! addresses the row it already created. The handler this replaces minted a
//! fresh `UUIDv7` per call, which meant its `attribute_not_exists(pk)` guard could
//! never fail: a retried request created a second export and a second Fargate
//! task, silently, and the caller was told nothing. Deriving it from the request
//! body instead would collide two deliberate exports of the same window, which
//! is why the operation id and not the body is the identity.
//!
//! **Admission pins the whole read plan.** The launcher passes the task exactly
//! two environment variables — the export id and the workspace — so the row is
//! the only channel there is. Anything the task needs and the row does not carry
//! is not "derived later", it is missing: the task's lease decode hard-requires
//! `scopeKey`, `format`, `completeness`, `expiresAt`, `fence`, `snapshot` and
//! `deletionEpochPinned`, and walks `partitions`. Letting the task derive the
//! snapshot and the partitions instead would let two runs of one export produce
//! different artifacts under the same manifest claim, and a resumed run walk a
//! different partition set than the one it checkpointed against.
//!
//! **Every row this plan writes commits together or not at all.** An export with
//! no operation row answers 404 on the client's first poll; an operation row
//! with no export is a durable identity nothing will ever run. Both are broken
//! contracts rather than lagging ones, which is why this is one transaction and
//! not a convergence.

use std::collections::BTreeMap;

use aex_observation_domain::keys::{ControlDomain, control_pk, control_sk, export_pk, export_sk};
use aex_wire::ids::{ExportId, OperationId, PrefixedId as _, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

/// How long an export artifact stays downloadable after it is admitted.
///
/// Seven days. The artifact is reproducible from the pinned plan, so the
/// retention is a convenience window rather than a durability guarantee, and a
/// shorter one costs a caller nothing but a re-request.
pub const EXPORT_RETENTION_MILLIS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// Why an export could not be admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportPlanError {
    /// A derived instant was not representable.
    #[error("the export retention deadline is not representable")]
    Unrepresentable,
    /// The pinned partition list was empty.
    ///
    /// Refused rather than admitted: an export with no partitions walks nothing
    /// and publishes an empty artifact that looks exactly like a complete one.
    #[error("an export must pin at least one partition to walk")]
    NoPartitions,
}

/// The scope one export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// Every observation the workspace owns.
    Workspace,
    /// One session's observations. Still workspace-owned.
    Session(SessionId),
}

impl ExportScope {
    /// The session this scope names, when it names one.
    #[must_use]
    pub const fn session(self) -> Option<SessionId> {
        match self {
            Self::Workspace => None,
            Self::Session(session) => Some(session),
        }
    }
}

/// Everything admission needs, already validated at the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportAdmission {
    /// The caller-minted operation identity.
    pub operation: OperationId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// What the export covers.
    pub scope: ExportScope,
    /// The artifact format, already checked admissible against the signal.
    pub format: String,
    /// What to do about recorded gaps.
    pub completeness: String,
    /// The normalized query, in the exact form the manifest hashes.
    pub normalized_query: String,
    /// The pinned, ordered partition list the walk visits.
    pub partitions: Vec<String>,
    /// The deletion epoch the scope was at when the export was admitted.
    pub deletion_epoch: u64,
    /// When admission happened.
    pub now: Timestamp,
}

/// One row a durable admission must write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRow {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
    /// The string attributes.
    pub text: BTreeMap<String, String>,
    /// The numeric attributes.
    pub numbers: BTreeMap<String, u64>,
    /// The boolean attributes.
    pub flags: BTreeMap<String, bool>,
    /// The ordered list attributes.
    pub lists: BTreeMap<String, Vec<String>>,
}

/// The whole admission, as one atomic transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    /// The derived export identity.
    pub export: ExportId,
    /// The export state row in `observation-authority`.
    pub export_row: ExportRow,
    /// When the artifact stops being downloadable.
    pub expires_at: Timestamp,
    /// The snapshot position the walk is pinned to, in epoch milliseconds.
    pub snapshot: i64,
}

/// Derives the export identity from the workspace and the operation id.
///
/// A pure function of the two, so a replayed operation id addresses the row it
/// already created and the row's `attribute_not_exists` guard becomes a real
/// replay detector instead of a condition that can never fail.
///
/// Export ids therefore stop being time-ordered, and any listing sorts by
/// `createdAt` rather than by key.
#[must_use]
pub fn derive_export_id(workspace: WorkspaceId, operation: OperationId) -> ExportId {
    use sha2::Digest as _;

    let mut digest = sha2::Sha256::new();
    digest.update(b"aex.observation.export.identity.v1\0");
    digest.update(workspace.to_string().as_bytes());
    digest.update(b"\0");
    digest.update(operation.to_string().as_bytes());
    let bytes: [u8; 32] = digest.finalize().into();
    // The identity is a digest, not a clock reading, so the timestamp field
    // carries digest bytes too. Export ids therefore stop being time-ordered
    // and any listing sorts by `createdAt` rather than by key.
    let millis = u64::from_be_bytes([
        0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
    ]);
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&bytes[6..16]);
    ExportId::from_uuid7(aex_wire::Uuid7::compose(millis, entropy))
}

/// Builds the durable admission plan.
///
/// # Errors
///
/// Returns [`ExportPlanError::NoPartitions`] for an empty partition list and
/// [`ExportPlanError::Unrepresentable`] when the retention deadline cannot be
/// represented.
pub fn plan(request: &ExportAdmission) -> Result<ExportPlan, ExportPlanError> {
    if request.partitions.is_empty() {
        return Err(ExportPlanError::NoPartitions);
    }
    let export = derive_export_id(request.workspace, request.operation);
    let expires_at = Timestamp::from_unix_millis(
        request
            .now
            .unix_millis()
            .checked_add(
                i64::try_from(EXPORT_RETENTION_MILLIS)
                    .map_err(|_| ExportPlanError::Unrepresentable)?,
            )
            .ok_or(ExportPlanError::Unrepresentable)?,
    )
    .map_err(|_| ExportPlanError::Unrepresentable)?;
    // The snapshot is taken at admission, not at launch. A long-queued export
    // therefore reflects the moment it was requested rather than the moment it
    // ran, which is the correct semantic for an export and the only one under
    // which two runs of it produce the same artifact.
    let snapshot = request.now.unix_millis();

    let scope_key = match request.scope {
        ExportScope::Workspace => format!("W#{}", request.workspace),
        ExportScope::Session(session) => format!("S#{}#{session}", request.workspace),
    };

    let mut text = BTreeMap::from([
        ("itemType".to_owned(), "export".to_owned()),
        ("exportId".to_owned(), export.to_string()),
        ("operationId".to_owned(), request.operation.to_string()),
        ("workspaceId".to_owned(), request.workspace.to_string()),
        ("scopeKey".to_owned(), scope_key),
        ("format".to_owned(), request.format.clone()),
        ("completeness".to_owned(), request.completeness.clone()),
        ("state".to_owned(), "admitted".to_owned()),
        // The client token the launcher passes to `RunTask`, so an ambiguous
        // launch is reconciled by identity rather than by a second run.
        ("clientToken".to_owned(), export.to_string()),
        ("createdAt".to_owned(), request.now.to_wire()),
        ("expiresAt".to_owned(), expires_at.to_wire()),
        (
            "normalizedQuery".to_owned(),
            request.normalized_query.clone(),
        ),
        // The sparse due-index attributes the launcher's next sweep finds. This
        // is the whole handoff: no queue, no EventBridge rule on the row and no
        // synchronous `RunTask`, because the transaction already carries the
        // signal and a queue delivery is never authority.
        ("cPk".to_owned(), control_pk(ControlDomain::ExportLaunch, 0)),
        (
            "cSk".to_owned(),
            control_sk(request.now, &export.to_string()),
        ),
        // TTL is disabled on this table by design: a TTL delete is unordered,
        // unfenced and invisible to the fenced duties. Expiry runs through a
        // reap due item instead, and until this was written nothing enqueued
        // one — which is why `ExportStatus::Expired` was read by the decoder and
        // written by nothing in the entire tree.
        ("rPk".to_owned(), control_pk(ControlDomain::ExportReap, 0)),
        (
            "rSk".to_owned(),
            control_sk(expires_at, &export.to_string()),
        ),
    ]);
    if let Some(session) = request.scope.session() {
        text.insert("sessionId".to_owned(), session.to_string());
    }

    Ok(ExportPlan {
        export,
        export_row: ExportRow {
            pk: export_pk(request.workspace),
            sk: export_sk(export),
            text,
            numbers: BTreeMap::from([
                ("fence".to_owned(), 0),
                ("deletionEpochPinned".to_owned(), request.deletion_epoch),
                (
                    "snapshot".to_owned(),
                    u64::try_from(snapshot.max(0)).unwrap_or(0),
                ),
            ]),
            flags: BTreeMap::from([("cancelRequested".to_owned(), false)]),
            lists: BTreeMap::from([("partitions".to_owned(), request.partitions.clone())]),
        },
        expires_at,
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::{ExportAdmission, ExportPlanError, ExportScope, derive_export_id, plan};
    use aex_wire::Uuid7;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, WorkspaceId};
    use aex_wire::types::Timestamp;

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn operation() -> OperationId {
        OperationId::from_uuid7(Uuid7::compose(2, [2; 10]))
    }

    fn at() -> Timestamp {
        Timestamp::from_unix_millis(1_785_000_000_000).expect("representable")
    }

    fn admission(scope: ExportScope) -> ExportAdmission {
        ExportAdmission {
            operation: operation(),
            workspace: workspace(),
            scope,
            format: "ndjson".to_owned(),
            completeness: "allow_gaps".to_owned(),
            normalized_query: "{\"signal\":\"logs\"}".to_owned(),
            partitions: vec!["OBS#W#ws#logs#2026-08-01".to_owned()],
            deletion_epoch: 3,
            now: at(),
        }
    }

    #[test]
    fn the_export_identity_is_a_function_of_the_workspace_and_the_operation() {
        // The whole point: a replayed operation id addresses the row it already
        // created, so the row's `attribute_not_exists` guard is a real replay
        // detector rather than a condition that can never fail.
        assert_eq!(
            derive_export_id(workspace(), operation()),
            derive_export_id(workspace(), operation())
        );
        let other = OperationId::from_uuid7(Uuid7::compose(3, [3; 10]));
        assert_ne!(
            derive_export_id(workspace(), operation()),
            derive_export_id(workspace(), other)
        );
        let elsewhere = WorkspaceId::from_uuid7(Uuid7::compose(4, [4; 10]));
        assert_ne!(
            derive_export_id(workspace(), operation()),
            derive_export_id(elsewhere, operation()),
            "one operation id in two workspaces is two exports"
        );
    }

    #[test]
    fn the_identity_does_not_depend_on_the_request_body() {
        // Deriving it from the body would collide two deliberate exports of the
        // same window, which is a data-loss bug rather than an idempotency one.
        let one = plan(&admission(ExportScope::Workspace)).expect("plans");
        let mut different = admission(ExportScope::Workspace);
        different.format = "otlp_json".to_owned();
        different.normalized_query = "{\"signal\":\"spans\"}".to_owned();
        let other = plan(&different).expect("plans");
        assert_eq!(one.export, other.export);
    }

    #[test]
    fn an_admitted_row_carries_everything_the_task_cannot_derive() {
        let planned = plan(&admission(ExportScope::Workspace)).expect("plans");
        let row = &planned.export_row;
        // The launcher passes the task only the export id and the workspace, so
        // anything absent here is missing rather than derived later. This is
        // exactly the set the task's lease decode hard-requires.
        for attribute in [
            "scopeKey",
            "format",
            "completeness",
            "expiresAt",
            "normalizedQuery",
        ] {
            assert!(
                row.text.contains_key(attribute),
                "`{attribute}` is absent, so the task's first lease read fails"
            );
        }
        for attribute in ["fence", "snapshot", "deletionEpochPinned"] {
            assert!(row.numbers.contains_key(attribute), "`{attribute}`");
        }
        assert!(
            !row.lists["partitions"].is_empty(),
            "an export with no partitions walks nothing and publishes an empty \
             artifact that looks exactly like a complete one"
        );
        assert!(!row.flags["cancelRequested"]);
        assert_eq!(row.text["state"], "admitted");
        // The client token is the export identity, so an ambiguous launch is
        // reconciled by identity rather than by a second `RunTask`.
        assert_eq!(row.text["clientToken"], planned.export.to_string());
    }

    #[test]
    fn admission_enqueues_both_the_launch_and_the_reap() {
        let planned = plan(&admission(ExportScope::Workspace)).expect("plans");
        let row = &planned.export_row;
        // The launch handoff is the due-index attribute and nothing else: the
        // launcher's next sweep finds it, with no queue in between.
        assert!(row.text["cPk"].contains("export.launch"));
        assert!(row.text["cSk"].starts_with(&at().to_wire()));
        // TTL is disabled on this table by design, so expiry has to be enqueued
        // or `ExportStatus::Expired` stays a state nothing can ever write.
        assert!(row.text["rPk"].contains("export.reap"));
        assert!(row.text["rSk"].starts_with(&planned.expires_at.to_wire()));
        assert!(planned.expires_at.unix_millis() > at().unix_millis());
    }

    #[test]
    fn a_session_export_is_workspace_owned_and_names_its_session() {
        let session = SessionId::from_uuid7(Uuid7::compose(5, [5; 10]));
        let planned = plan(&admission(ExportScope::Session(session))).expect("plans");
        let row = &planned.export_row;
        assert_eq!(row.pk, super::export_pk(workspace()));
        assert_eq!(row.text["sessionId"], session.to_string());
        assert!(row.text["scopeKey"].starts_with("S#"));
        // Same partition, same row shape, one scope parameter: the two routes
        // are one problem seen twice, not two problems.
        let workspace_scoped = plan(&admission(ExportScope::Workspace)).expect("plans");
        assert_eq!(row.pk, workspace_scoped.export_row.pk);
        assert!(workspace_scoped.export_row.text["scopeKey"].starts_with("W#"));
        assert!(!workspace_scoped.export_row.text.contains_key("sessionId"));
    }

    #[test]
    fn the_snapshot_is_pinned_at_admission_rather_than_at_launch() {
        // A long-queued export reflects the moment it was requested. Letting the
        // task pick would make two runs of one export produce different
        // artifacts under the same manifest claim.
        let planned = plan(&admission(ExportScope::Workspace)).expect("plans");
        assert_eq!(planned.snapshot, at().unix_millis());
    }

    #[test]
    fn an_export_with_nothing_to_walk_is_refused_rather_than_admitted() {
        let mut empty = admission(ExportScope::Workspace);
        empty.partitions.clear();
        assert_eq!(plan(&empty), Err(ExportPlanError::NoPartitions));
    }
}
