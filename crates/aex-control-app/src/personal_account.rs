//! Atomic first-login provisioning for the one launch account and workspace.
//!
//! Identity resolution happens first because the provider subject owns the
//! stable user identity. This ceremony then commits the personal-account link,
//! owner membership, hidden regional-workspace intent, finance accounts and
//! projection outbox in one Aurora transaction. A retry is keyed by `user_id`,
//! not by one of the fresh candidate ids, so an ambiguous response cannot
//! allocate a second account.

use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use aex_control_domain::IntentHash;

use crate::ports::{IdFactory, ReconcileIdentity, StoreError, TxOutcome, UnknownCommit};
use crate::use_cases::ControlError;

/// The candidate identities for every durable row first login may create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonalAccountProvisionIds {
    /// The personal account and internal organization identity.
    pub account_id: Uuid,
    /// The fixed owner membership.
    pub membership_id: Uuid,
    /// The one launch workspace.
    pub workspace_id: Uuid,
    /// The durable regional-provision operation.
    pub operation_id: Uuid,
    /// The worker's replay row.
    pub idempotency_id: Uuid,
    /// The regional-projection outbox row.
    pub outbox_id: Uuid,
    /// The immutable first-login audit row.
    pub audit_id: Uuid,
    /// The prepaid available-balance ledger account.
    pub available_account_id: Uuid,
    /// The prepaid reserved-balance ledger account.
    pub reserved_account_id: Uuid,
}

impl PersonalAccountProvisionIds {
    /// Mints every candidate through the repository's one `UUIDv7` owner.
    #[must_use]
    pub fn mint(ids: &dyn IdFactory) -> Self {
        Self {
            account_id: ids.next(),
            membership_id: ids.next(),
            workspace_id: ids.next(),
            operation_id: ids.next(),
            idempotency_id: ids.next(),
            outbox_id: ids.next(),
            audit_id: ids.next(),
            available_account_id: ids.next(),
            reserved_account_id: ids.next(),
        }
    }

    /// Every candidate, for validation and diagnostics that need no field map.
    #[must_use]
    pub fn all(self) -> Vec<Uuid> {
        vec![
            self.account_id,
            self.membership_id,
            self.workspace_id,
            self.operation_id,
            self.idempotency_id,
            self.outbox_id,
            self.audit_id,
            self.available_account_id,
            self.reserved_account_id,
        ]
    }
}

/// One idempotent first-login transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalAccountProvisionCommand {
    /// The already-resolved provider subject.
    pub user_id: Uuid,
    /// Candidate row identities, used only when this user is new to the account
    /// authority.
    pub ids: PersonalAccountProvisionIds,
    /// Stable intent for the fixed account/workspace policy.
    pub intent_hash: IntentHash,
    /// The request instant shared by every inserted row.
    pub now: OffsetDateTime,
}

impl PersonalAccountProvisionCommand {
    fn new(ids: &dyn IdFactory, user_id: Uuid, now: OffsetDateTime) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"aex/personal-account/eu-west-1\x1f");
        digest.update(user_id.as_bytes());
        Self {
            user_id,
            ids: PersonalAccountProvisionIds::mint(ids),
            intent_hash: IntentHash::from_bytes(digest.finalize().into()),
            now,
        }
    }
}

/// The authoritative identities first login resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalAccountProvision {
    /// The account identity exposed by bootstrap and billing.
    pub account_id: Uuid,
    /// Its sole owner.
    pub user_id: Uuid,
    /// The internal owner-membership identity.
    pub membership_id: Uuid,
    /// The fixed launch workspace.
    pub workspace_id: Uuid,
    /// Its available-balance ledger account.
    pub available_account_id: Uuid,
    /// Its reserved-balance ledger account.
    pub reserved_account_id: Uuid,
    /// When first login created the aggregate.
    pub created_at: OffsetDateTime,
}

/// Aurora's one-transaction personal-account authority.
#[async_trait]
pub trait PersonalAccountProvisioner: Send + Sync {
    /// Creates or resolves the one aggregate for `command.user_id`.
    ///
    /// # Errors
    ///
    /// Returns a typed store failure without retrying under a new user identity.
    async fn provision_personal_account(
        &self,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<TxOutcome<PersonalAccountProvision>, StoreError>;
}

/// The first-login application ceremony.
pub struct ProvisionPersonalAccount;

impl ProvisionPersonalAccount {
    /// Mints candidates, then atomically creates or resolves the user's account.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError`] with the stable user reconciliation identity on
    /// an ambiguous commit.
    pub async fn run(
        provisioner: &dyn PersonalAccountProvisioner,
        ids: &dyn IdFactory,
        user_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<PersonalAccountProvision, ControlError> {
        let command = PersonalAccountProvisionCommand::new(ids, user_id, now);
        match provisioner.provision_personal_account(&command).await? {
            TxOutcome::Committed(account) | TxOutcome::Replayed(account) => Ok(account),
            TxOutcome::IntentConflict => Err(ControlError::IntentConflict),
            TxOutcome::Unknown(UnknownCommit {
                identity: ReconcileIdentity { ceremony, id },
            }) => Err(ControlError::CommitOutcomeUnknown { ceremony, id }),
        }
    }
}

/// Stable reconciliation name used by the Aurora adapter.
pub const CEREMONY: &str = "control.provision_personal_account";
