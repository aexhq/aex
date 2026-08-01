//! The account pause gate.
//!
//! `project_account` keeps the higher revision and rejects a regression, so
//! applying projections in any order converges. `pause_gate` denies every
//! pausable command with `402 account_paused`; the exempt set is closed and
//! small, and it is exempt because those are exactly the commands a customer
//! needs in order to stop spending or to pay.
//!
//! Already-admitted work is not killed. It fences at its next prepaid segment
//! boundary as `Interrupted(AccountPaused)`, and clearing a pause restores
//! access only — there is no resume transition.

use aex_wire::error::ErrorCode;
use aex_wire::ids::OrganizationId;
use aex_wire::types::Timestamp;

use crate::ids::AccountRevision;

/// Why an account is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PauseReason {
    /// The prepaid balance needs topping up.
    TopUpRequired,
}

/// Whether an account may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountState {
    /// Spending is allowed.
    Active,
    /// Spending is blocked.
    Paused {
        /// Why.
        reason: PauseReason,
    },
}

impl AccountState {
    /// Whether the account is paused.
    #[must_use]
    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Paused { .. })
    }
}

/// The regional projection of one organization's account state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountProjection {
    /// Which organization.
    pub organization: OrganizationId,
    /// The projection's revision.
    pub revision: AccountRevision,
    /// Whether it may spend.
    pub state: AccountState,
    /// When the projection was taken.
    pub observed_at: Timestamp,
}

/// Merges an incoming projection into the one already held.
///
/// The higher revision always wins and a regression is discarded, so applying
/// any permutation of the same projections converges to the same value. That is
/// what makes an out-of-order delivery harmless rather than a correctness bug.
#[must_use]
pub fn project_account(
    prior: &AccountProjection,
    incoming: &AccountProjection,
) -> AccountProjection {
    if incoming.organization != prior.organization || incoming.revision <= prior.revision {
        return *prior;
    }
    *incoming
}

/// What a command costs an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandClass {
    /// A mutation that can spend.
    PausableMutation,
    /// A read that can spend.
    PausableRead,
    /// A command that must stay available while paused.
    PauseExempt,
}

impl CommandClass {
    /// Every class, in canonical order.
    pub const ALL: [Self; 3] = [
        Self::PausableMutation,
        Self::PausableRead,
        Self::PauseExempt,
    ];
}

/// The closed exempt set.
///
/// Security revocation, stop, discard, trash and purge; account and workspace
/// state reads; billing and payment; and the safe operation envelopes for those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExemptCommand {
    /// Revoke a secret or a credential.
    SecurityRevocation,
    /// Stop a session's work.
    SessionStop,
    /// Discard the live workspace.
    WorkspaceDiscard,
    /// Trash a session.
    SessionTrash,
    /// Purge a session or a workspace.
    Purge,
    /// Read account or workspace state.
    StateRead,
    /// Billing and payment.
    Billing,
    /// The operation envelope of one of the above.
    OperationEnvelope,
}

impl ExemptCommand {
    /// Every exempt command. The set is closed; a command not on it is pausable.
    pub const ALL: [Self; 8] = [
        Self::SecurityRevocation,
        Self::SessionStop,
        Self::WorkspaceDiscard,
        Self::SessionTrash,
        Self::Purge,
        Self::StateRead,
        Self::Billing,
        Self::OperationEnvelope,
    ];

    /// The class an exempt command carries.
    #[must_use]
    pub const fn class(self) -> CommandClass {
        CommandClass::PauseExempt
    }
}

/// Why a command was refused while paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("account is paused: {reason:?}")]
pub struct PauseRejection {
    /// Why the account is paused.
    pub reason: PauseReason,
    /// The stable public code.
    pub code: ErrorCode,
}

/// Whether a command of this class may run right now.
///
/// # Errors
///
/// Returns [`PauseRejection`] with `account_paused` for every pausable command
/// while the account is paused.
pub const fn pause_gate(
    class: CommandClass,
    account: &AccountProjection,
) -> Result<(), PauseRejection> {
    match (class, account.state) {
        (CommandClass::PauseExempt, _) | (_, AccountState::Active) => Ok(()),
        (
            CommandClass::PausableMutation | CommandClass::PausableRead,
            AccountState::Paused { reason },
        ) => Err(PauseRejection {
            reason,
            code: ErrorCode::AccountPaused,
        }),
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::error::ErrorCode;
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{
        AccountProjection, AccountState, CommandClass, ExemptCommand, PauseReason, pause_gate,
        project_account,
    };
    use crate::ids::AccountRevision;

    fn organization(tag: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn projection(revision: u64, state: AccountState) -> AccountProjection {
        AccountProjection {
            organization: organization(1),
            revision: AccountRevision(revision),
            state,
            observed_at: Timestamp::from_unix_millis(i64::try_from(revision).expect("small"))
                .expect("in range"),
        }
    }

    #[test]
    fn the_higher_revision_always_wins() {
        let low = projection(1, AccountState::Active);
        let high = projection(
            5,
            AccountState::Paused {
                reason: PauseReason::TopUpRequired,
            },
        );
        assert_eq!(project_account(&low, &high), high);
        assert_eq!(project_account(&high, &low), high);
        assert_eq!(project_account(&high, &high), high);
    }

    #[test]
    fn a_foreign_organization_is_discarded() {
        let mine = projection(1, AccountState::Active);
        let foreign = AccountProjection {
            organization: organization(9),
            revision: AccountRevision(99),
            ..mine
        };
        assert_eq!(project_account(&mine, &foreign), mine);
    }

    #[test]
    fn the_exempt_set_is_exactly_the_declared_list() {
        let paused = projection(
            1,
            AccountState::Paused {
                reason: PauseReason::TopUpRequired,
            },
        );
        for command in ExemptCommand::ALL {
            assert_eq!(pause_gate(command.class(), &paused), Ok(()));
        }
        for class in [CommandClass::PausableMutation, CommandClass::PausableRead] {
            let rejection = pause_gate(class, &paused).expect_err("must deny");
            assert_eq!(rejection.code, ErrorCode::AccountPaused);
            assert_eq!(rejection.reason, PauseReason::TopUpRequired);
        }
    }

    #[test]
    fn an_active_account_admits_every_class() {
        let active = projection(1, AccountState::Active);
        for class in CommandClass::ALL {
            assert_eq!(pause_gate(class, &active), Ok(()));
        }
    }
}
