//! The control-plane commands.
//!
//! Workspace provisioning is the interesting one, and it is deliberately three
//! explicit steps rather than one implicit ceremony:
//!
//! ```text
//! T1  workspace 'provisioning' (hidden) + internal operation + replay record
//!     + audit + outbox                                              COMMIT
//! E   RegionalControlPort::provision_workspace, no transaction held
//! T2  workspace -> 'active', operation -> 'succeeded', replay -> completed
//!     + audit                                                       COMMIT -> 201
//! ```
//!
//! `201` is returned only once both authorities are durable. A failure or lost
//! response anywhere in `E` or `T2` answers `workspace_provision_pending`,
//! leaves the workspace hidden, and lets the same `Idempotency-Key` retry the
//! identical intent. The workspace id is preassigned, so the reconciler asks the
//! region about *that* workspace rather than creating a second one.

use time::OffsetDateTime;
use uuid::Uuid;

use aex_control_domain::{Operation, Organization, Workspace};

use crate::ports::{
    AcceptInvitationsTx, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx, ControlStore,
    CreateApiKeyTx, CreateInvitationTx, CreateOrganizationTx, DeleteWorkspaceRequest, EffectError,
    FinishWorkspaceProvisionTx, ProvisionWorkspaceRequest, RegionalControlPort, RevokeApiKeyTx,
    StoreError, TxOutcome,
};

/// Why a control command did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    /// A uniqueness or state guard refused the write.
    #[error("conflict on `{constraint}`")]
    Conflict {
        /// The exact database constraint name.
        constraint: String,
    },
    /// Nothing matched.
    #[error("not found")]
    NotFound,
    /// The same replay key was reused with a different intent.
    #[error("the replay key was reused with a different intent")]
    IntentConflict,
    /// The command is still running under the same identity.
    #[error("the same identity is already in flight")]
    InFlight,
    /// Removing this membership would leave the organization with no owner.
    #[error("an organization always has at least one active owner")]
    LastOwnerRequired,
    /// The operation cannot be cancelled.
    #[error("the operation is not cancelable")]
    NotCancelable,
    /// A deletion of this resource is already running.
    #[error("a deletion is already in progress")]
    DeletionInProgress,
    /// The regional half is not durable yet. **Retry the same intent.**
    #[error("the workspace is not provisioned yet; retry the same Idempotency-Key")]
    WorkspaceProvisionPending,
    /// The store could not be reached; nothing was applied.
    #[error("the control store is unavailable")]
    Unavailable,
    /// The commit outcome is unknown. **Retry the same identity.**
    #[error("the commit outcome is unknown; reconcile `{ceremony}` for `{id}`")]
    CommitOutcomeUnknown {
        /// Which ceremony to reconcile.
        ceremony: &'static str,
        /// The identity it preassigned.
        id: Uuid,
    },
    /// Anything unrecoverable.
    #[error("fatal: {0}")]
    Fatal(String),
}

impl ControlError {
    /// Whether the caller may retry the same identity.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Unavailable
                | Self::CommitOutcomeUnknown { .. }
                | Self::WorkspaceProvisionPending
                | Self::InFlight
        )
    }
}

impl From<StoreError> for ControlError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Conflict { constraint } => Self::Conflict { constraint },
            StoreError::NotFound => Self::NotFound,
            StoreError::Unavailable => Self::Unavailable,
            StoreError::Unknown => Self::Fatal("the store outcome is unknown".to_owned()),
            StoreError::Decode(reason) | StoreError::Fatal(reason) => Self::Fatal(reason),
            StoreError::PermissionDenied => Self::Fatal("permission denied".to_owned()),
        }
    }
}

/// The named ceremonies, for reconciliation after a lost commit.
pub mod ceremony {
    /// `POST /api/organizations`.
    pub const CREATE_ORGANIZATION: &str = "control.create_organization";
    /// `POST /api/organizations/{org}/invitations`.
    pub const CREATE_INVITATION: &str = "control.create_invitation";
    /// `POST /api/invitations/acceptances`.
    pub const ACCEPT_INVITATIONS: &str = "control.accept_invitations";
    /// `POST /api/workspaces`, first half.
    pub const BEGIN_WORKSPACE_PROVISION: &str = "control.begin_workspace_provision";
    /// `POST /api/workspaces`, second half.
    pub const FINISH_WORKSPACE_PROVISION: &str = "control.finish_workspace_provision";
    /// `POST /api/workspaces/{wsp}/deletions`.
    pub const BEGIN_WORKSPACE_DELETION: &str = "control.begin_workspace_deletion";
    /// `POST /api/api-keys`.
    pub const CREATE_API_KEY: &str = "control.create_api_key";
}

/// Maps one atomic outcome onto the application's error vocabulary.
fn settle<T>(outcome: TxOutcome<T>) -> Result<(T, bool), ControlError> {
    match outcome {
        TxOutcome::Committed(value) => Ok((value, true)),
        TxOutcome::Replayed(value) => Ok((value, false)),
        TxOutcome::IntentConflict => Err(ControlError::IntentConflict),
        TxOutcome::Unknown(unknown) => Err(ControlError::CommitOutcomeUnknown {
            ceremony: unknown.identity.ceremony,
            id: unknown.identity.id,
        }),
    }
}

/// Create an organization.
pub struct CreateOrganization;

impl CreateOrganization {
    /// Runs the command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure.
    pub async fn run(
        store: &dyn ControlStore,
        command: &CreateOrganizationTx,
    ) -> Result<Organization, ControlError> {
        let outcome = store.create_organization(command).await?;
        settle(outcome).map(|(organization, _)| organization)
    }
}

/// Invite somebody to an organization.
pub struct CreateInvitation;

impl CreateInvitation {
    /// Runs the command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure. The notification is an
    /// outbox row committed with the invitation, so the invitation exists
    /// whether or not the mail ever leaves.
    pub async fn run(
        store: &dyn ControlStore,
        command: &CreateInvitationTx,
    ) -> Result<aex_control_domain::Invitation, ControlError> {
        let outcome = store.create_invitation(command).await?;
        settle(outcome).map(|(invitation, _)| invitation)
    }
}

/// Redeem every pending invitation matching a verified address.
pub struct AcceptInvitations;

impl AcceptInvitations {
    /// Runs the command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure. An unverified address
    /// yields an empty result rather than a membership, because the address is
    /// the whole proof.
    pub async fn run(
        store: &dyn ControlStore,
        command: &AcceptInvitationsTx,
    ) -> Result<Vec<aex_control_domain::Membership>, ControlError> {
        if !command.email_verified {
            return Ok(Vec::new());
        }
        let outcome = store.accept_invitations_for_email(command).await?;
        settle(outcome).map(|(memberships, _)| memberships)
    }
}

/// What provisioning produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedWorkspace {
    /// The workspace, now active and visible.
    pub workspace: Workspace,
    /// The internal reconciliation anchor.
    pub operation: Operation,
}

/// Create a workspace across both authorities.
pub struct CreateWorkspace;

impl CreateWorkspace {
    /// Runs the three-step ceremony.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError::WorkspaceProvisionPending`] whenever the regional
    /// half is not durable — including when its outcome is unknown — so the
    /// workspace stays hidden and the same `Idempotency-Key` retries the
    /// identical intent. It never returns a different workspace and never
    /// returns a `201` for a workspace only one authority knows about.
    pub async fn run(
        store: &dyn ControlStore,
        regional: &dyn RegionalControlPort,
        begin: &BeginWorkspaceProvisionTx,
        response_body: serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<ProvisionedWorkspace, ControlError> {
        // T1: the central half, committed and hidden.
        let outcome = store.begin_workspace_provision(begin).await?;
        let ((workspace, operation), _) = settle(outcome)?;

        // E: the regional half, with no transaction held.
        let request = ProvisionWorkspaceRequest {
            workspace_id: workspace.id,
            organization_id: workspace.organization_id,
            region: workspace.region,
            fence: operation.fence,
            intent_hash: operation.intent_hash,
        };
        let response = match regional.provision_workspace(&request).await {
            Ok(response) => response,
            // Unknown is never converted into failure: the region may have
            // created it, so the reconciler asks about this exact workspace id.
            // Unavailable, unknown and a retryable rejection all leave the
            // workspace hidden and the same intent retryable; only a permanent
            // rejection is a failure the caller cannot fix by trying again.
            Err(
                EffectError::Unknown
                | EffectError::Unavailable
                | EffectError::Rejected {
                    retryable: true, ..
                },
            ) => {
                return Err(ControlError::WorkspaceProvisionPending);
            }
            Err(EffectError::Rejected { code, .. }) => {
                return Err(ControlError::Fatal(code.to_owned()));
            }
        };
        if response.workspace_id != workspace.id {
            return Err(ControlError::Fatal(
                "the region answered about a different workspace".to_owned(),
            ));
        }

        // T2: both halves durable.
        let finish = FinishWorkspaceProvisionTx {
            workspace_id: workspace.id,
            operation_id: operation.id,
            fence: operation.fence,
            idempotency_id: begin.idempotency.id,
            response_body,
            audit: aex_control_domain::AuditEvent {
                id: begin.completion_audit_id,
                ..begin.audit.clone()
            },
            now,
        };
        match store.finish_workspace_provision(&finish).await {
            Ok(outcome) => {
                let (workspace, _) = settle(outcome)?;
                Ok(ProvisionedWorkspace {
                    workspace,
                    operation,
                })
            }
            Err(error) if error.retryable() => Err(ControlError::WorkspaceProvisionPending),
            Err(error) => Err(error.into()),
        }
    }
}

/// Accept a workspace deletion.
pub struct DeleteWorkspace;

impl DeleteWorkspace {
    /// Runs the acceptance, which closes admission before any irreversible
    /// regional cleanup: every key for the workspace is revoked and both epochs
    /// advance inside `T1`.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure.
    pub async fn run(
        store: &dyn ControlStore,
        command: &BeginWorkspaceDeletionTx,
    ) -> Result<(Workspace, Operation), ControlError> {
        let outcome = store.begin_workspace_deletion(command).await?;
        settle(outcome).map(|(pair, _)| pair)
    }

    /// Dispatches the fenced regional half. Run by the worker, never by the API.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure; an unknown outcome leaves
    /// the operation claimable and is retried under a fresh fence.
    pub async fn dispatch(
        store: &dyn ControlStore,
        regional: &dyn RegionalControlPort,
        workspace: &Workspace,
        operation: &Operation,
        now: OffsetDateTime,
    ) -> Result<(), ControlError> {
        let request = DeleteWorkspaceRequest {
            workspace_id: workspace.id,
            region: workspace.region,
            fence: operation.fence,
        };
        match regional.delete_workspace(&request).await {
            // `removed = false` is still the authoritative postcondition: the
            // regional half is absent. This is the expected answer when a
            // response was lost after the first delete and the worker repeats
            // the exact fenced request.
            Ok(_) => {
                let complete = crate::ports::CompleteWorkspaceDeletionTx {
                    workspace_id: workspace.id,
                    operation_id: operation.id,
                    fence: operation.fence,
                    audit: audit_placeholder(workspace, now),
                    now,
                };
                let outcome = store.complete_workspace_deletion(&complete).await?;
                settle(outcome).map(|(_, _)| ())
            }
            Err(
                EffectError::Unknown
                | EffectError::Unavailable
                | EffectError::Rejected {
                    retryable: true, ..
                },
            ) => Err(ControlError::Unavailable),
            Err(EffectError::Rejected { code, .. }) => Err(ControlError::Fatal(code.to_owned())),
        }
    }
}

/// The audit row a worker-driven completion records.
fn audit_placeholder(workspace: &Workspace, now: OffsetDateTime) -> aex_control_domain::AuditEvent {
    aex_control_domain::AuditEvent {
        id: Uuid::nil(),
        organization_id: Some(workspace.organization_id),
        workspace_id: Some(workspace.id),
        actor_kind: aex_control_domain::ActorKind::System,
        actor_id: None,
        action: "workspace.delete.completed".to_owned(),
        resource_kind: aex_control_domain::ResourceKind::Workspace,
        resource_id: Some(workspace.id),
        outcome: aex_control_domain::AuditOutcome::Allowed,
        request_id: String::new(),
        operation_id: workspace.deletion_operation_id,
        detail: serde_json::json!({ "to_status": "deleted" }),
        occurred_at: now,
    }
}

/// Mint a workspace API key.
pub struct CreateApiKey;

/// The API-key metadata and whether this call performed the one plaintext-bearing mint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedApiKey {
    /// The durable metadata.
    pub key: aex_control_domain::ApiKey,
    /// `true` only for the transaction that first committed the verifier.
    pub first: bool,
}

impl CreateApiKey {
    /// Runs the command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure.
    pub async fn run(
        store: &dyn ControlStore,
        command: &CreateApiKeyTx,
    ) -> Result<CreatedApiKey, ControlError> {
        let outcome = store.create_api_key(command).await?;
        settle(outcome).map(|(key, first)| CreatedApiKey { key, first })
    }
}

/// Revoke a workspace API key.
pub struct RevokeApiKey;

impl RevokeApiKey {
    /// Runs the command, honouring `If-Match` when the caller supplied one.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] naming the failure. A mismatching `If-Match` is
    /// a conflict rather than a silently ignored header — the system this
    /// replaces parsed the header and then discarded it.
    pub async fn run(
        store: &dyn ControlStore,
        command: &RevokeApiKeyTx,
    ) -> Result<(), ControlError> {
        let outcome = store.revoke_api_key(command).await?;
        settle(outcome).map(|((), _)| ())
    }
}

/// Attempt to cancel a central operation.
pub struct CancelOperation;

impl CancelOperation {
    /// Runs the command.
    ///
    /// # Errors
    ///
    /// Always returns [`ControlError::NotCancelable`], and
    /// [`ControlError::NotFound`] for an operation that does not exist. Neither
    /// central operation kind is cancelable once accepted: by the time deletion
    /// is accepted the keys are already revoked, and provisioning returns a
    /// workspace rather than an operation. Saying so is honest; pretending
    /// otherwise is what the system this replaces did by accident.
    ///
    /// **A terminal operation refuses too.** It used to answer `200` with the
    /// row unchanged, which is the same response a cancellation that worked
    /// would produce — so a caller could not tell "I cancelled it" from "I did
    /// nothing." One refusal covers both states, and
    /// `409 operation_not_cancelable` ("the operation has passed its
    /// cancellation point") describes a finished operation exactly.
    pub async fn run(
        store: &dyn ControlStore,
        operation_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Operation, ControlError> {
        let Some(operation) = store.get_operation(operation_id).await? else {
            return Err(ControlError::NotFound);
        };
        match operation.cancel(now) {
            Ok(cancelled) => Ok(cancelled),
            Err(_) => Err(ControlError::NotCancelable),
        }
    }
}
