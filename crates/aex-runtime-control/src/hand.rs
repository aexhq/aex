//! One logical Hand per session and its eager preparation/readiness decisions.
//!
//! A Hand is the session-scoped logical sandbox. A generation is one concrete
//! provider incarnation of that Hand. Keeping those identities separate lets a
//! later replacement remain safe without permitting two usable sandboxes for a
//! session.

use aex_wire::ids::{GenerationId, SessionId};
use serde::{Deserialize, Serialize};

/// The one logical Hand identity for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandId(SessionId);

impl HandId {
    /// Derives the sole logical Hand from its owning session.
    #[must_use]
    pub const fn for_session(session: SessionId) -> Self {
        Self(session)
    }

    /// The owning session.
    #[must_use]
    pub const fn session(self) -> SessionId {
        self.0
    }
}

/// Durable preparation state for the logical Hand's current generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandState {
    /// Session explicitly disabled its sandbox. No provider generation exists.
    Disabled,
    /// Eager preparation was requested.
    Requested,
    /// The provider is allocating the generation.
    Provisioning,
    /// The guest is booting and binding its exact identity.
    Booting,
    /// The frozen workspace manifest is being materialized.
    MaterializingWorkspace,
    /// Exact generation is ready and may admit calls.
    Ready,
    /// The no-waiter suspend decision won and is in flight.
    Suspending,
    /// Provider retained the generation without running it.
    Suspended,
    /// A durable waiter caused a resume.
    Resuming,
    /// Provider or guest loss made this generation unusable.
    Lost,
    /// Session cleanup permanently ended this Hand.
    Terminated,
}

impl HandState {
    /// Whether no future transition may make this record usable.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Disabled | Self::Lost | Self::Terminated)
    }
}

/// The durable record over which preparation, waiters, and suspension race.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HandRecord {
    /// Session-derived logical identity.
    pub hand: HandId,
    /// Exact current provider generation; absent only when disabled.
    pub generation: Option<GenerationId>,
    /// Current lifecycle state.
    pub state: HandState,
    /// Durable tool calls waiting for or using readiness.
    pub waiters: u32,
    /// Optimistic concurrency revision.
    pub revision: u64,
}

/// Provider/control action owed after one accepted state mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandAction {
    /// No provider side effect; await the next durable event.
    Wait,
    /// Start provider allocation.
    Provision,
    /// Start guest boot validation.
    Boot,
    /// Materialize the frozen session workspace.
    MaterializeWorkspace,
    /// Suspend this exact generation.
    Suspend,
    /// Resume this exact generation.
    Resume,
    /// The exact generation is ready for the waiting call.
    Execute,
}

/// Why a Hand mutation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandDecisionError {
    /// An event was delivered to the wrong lifecycle state.
    #[error("Hand event is invalid while the Hand is {state:?}")]
    InvalidState {
        /// Observed state.
        state: HandState,
    },
    /// A call named a stale or foreign generation.
    #[error("tool call does not name the Hand's exact generation")]
    WrongGeneration,
    /// Explicit opt-out is a model-visible tool result, never a runtime launch.
    #[error("sandbox is disabled for this session")]
    SandboxDisabled,
    /// The durable waiter count would overflow.
    #[error("Hand waiter count is exhausted")]
    WaiterOverflow,
}

impl HandRecord {
    /// Creates the default-on or explicitly-disabled session Hand.
    #[must_use]
    pub const fn new(session: SessionId, generation: GenerationId, enabled: bool) -> Self {
        Self {
            hand: HandId::for_session(session),
            generation: if enabled { Some(generation) } else { None },
            state: if enabled {
                HandState::Requested
            } else {
                HandState::Disabled
            },
            waiters: 0,
            revision: 0,
        }
    }

    /// The first eager action. Disabled sessions owe no runtime call.
    #[must_use]
    pub fn eager_action(&mut self) -> HandAction {
        if !matches!(self.state, HandState::Requested) {
            return HandAction::Wait;
        }
        self.state = HandState::Provisioning;
        self.revision = self.revision.saturating_add(1);
        HandAction::Provision
    }

    /// Applies one successful preparation phase.
    ///
    /// # Errors
    ///
    /// Refuses an event that is not the unique successor of the current phase.
    pub fn phase_succeeded(&mut self) -> Result<HandAction, HandDecisionError> {
        let (next, action) = match self.state {
            HandState::Provisioning => (HandState::Booting, HandAction::Boot),
            HandState::Booting => (
                HandState::MaterializingWorkspace,
                HandAction::MaterializeWorkspace,
            ),
            HandState::MaterializingWorkspace | HandState::Resuming => {
                if self.waiters == 0 {
                    (HandState::Suspending, HandAction::Suspend)
                } else {
                    (HandState::Ready, HandAction::Execute)
                }
            }
            HandState::Suspending => {
                if self.waiters == 0 {
                    (HandState::Suspended, HandAction::Wait)
                } else {
                    (HandState::Resuming, HandAction::Resume)
                }
            }
            state => return Err(HandDecisionError::InvalidState { state }),
        };
        self.state = next;
        self.revision = self.revision.saturating_add(1);
        Ok(action)
    }

    /// Durably admits a call that needs this exact generation.
    ///
    /// # Errors
    ///
    /// Refuses disabled/terminal state, a generation mismatch, or waiter overflow.
    pub fn add_waiter(
        &mut self,
        generation: GenerationId,
    ) -> Result<HandAction, HandDecisionError> {
        if self.state == HandState::Disabled {
            return Err(HandDecisionError::SandboxDisabled);
        }
        if self.generation != Some(generation) {
            return Err(HandDecisionError::WrongGeneration);
        }
        if matches!(self.state, HandState::Lost | HandState::Terminated) {
            return Err(HandDecisionError::InvalidState { state: self.state });
        }
        self.waiters = self
            .waiters
            .checked_add(1)
            .ok_or(HandDecisionError::WaiterOverflow)?;
        self.revision = self.revision.saturating_add(1);
        match self.state {
            HandState::Ready => Ok(HandAction::Execute),
            HandState::Suspended => {
                self.state = HandState::Resuming;
                Ok(HandAction::Resume)
            }
            HandState::Requested
            | HandState::Provisioning
            | HandState::Booting
            | HandState::MaterializingWorkspace
            | HandState::Suspending
            | HandState::Resuming => Ok(HandAction::Wait),
            HandState::Disabled | HandState::Lost | HandState::Terminated => {
                unreachable!("terminal states were refused before the waiter mutation")
            }
        }
    }

    /// Settles one durable waiter and decides whether to suspend.
    #[must_use]
    pub fn settle_waiter(&mut self) -> HandAction {
        if self.waiters == 0 {
            return HandAction::Wait;
        }
        self.waiters = self.waiters.saturating_sub(1);
        self.revision = self.revision.saturating_add(1);
        if self.waiters == 0 && self.state == HandState::Ready {
            self.state = HandState::Suspending;
            HandAction::Suspend
        } else {
            HandAction::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HandAction, HandDecisionError, HandRecord, HandState};
    use aex_wire::ids::{GenerationId, PrefixedId as _, SessionId, Uuid7};
    use proptest::prelude::*;

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn generation(seed: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
    }

    fn advance_to_materializing(hand: &mut HandRecord) {
        assert_eq!(hand.eager_action(), HandAction::Provision);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Boot));
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::MaterializeWorkspace));
    }

    #[test]
    fn default_on_prepares_eagerly_then_suspends_without_a_waiter() {
        let mut hand = HandRecord::new(session(), generation(1), true);
        advance_to_materializing(&mut hand);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Suspend));
        assert_eq!(hand.state, HandState::Suspending);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Wait));
        assert_eq!(hand.state, HandState::Suspended);
    }

    #[test]
    fn a_waiter_during_setup_prevents_the_initial_suspend() {
        let exact = generation(1);
        let mut hand = HandRecord::new(session(), exact, true);
        advance_to_materializing(&mut hand);
        assert_eq!(hand.add_waiter(exact), Ok(HandAction::Wait));
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Execute));
        assert_eq!(hand.state, HandState::Ready);
    }

    #[test]
    fn a_waiter_that_loses_the_suspend_race_resumes_before_execution() {
        let exact = generation(1);
        let mut hand = HandRecord::new(session(), exact, true);
        advance_to_materializing(&mut hand);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Suspend));
        assert_eq!(hand.add_waiter(exact), Ok(HandAction::Wait));
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Resume));
        assert_eq!(hand.state, HandState::Resuming);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Execute));
    }

    #[test]
    fn disabled_and_stale_generation_calls_never_create_runtime_work() {
        let exact = generation(1);
        let mut disabled = HandRecord::new(session(), exact, false);
        assert_eq!(disabled.eager_action(), HandAction::Wait);
        assert_eq!(
            disabled.add_waiter(exact),
            Err(HandDecisionError::SandboxDisabled)
        );
        let mut enabled = HandRecord::new(session(), exact, true);
        assert_eq!(
            enabled.add_waiter(generation(2)),
            Err(HandDecisionError::WrongGeneration)
        );
        assert_eq!(enabled.waiters, 0);
    }

    #[test]
    fn a_cancelled_resume_suspends_again_without_exposing_readiness() {
        let exact = generation(1);
        let mut hand = HandRecord::new(session(), exact, true);
        advance_to_materializing(&mut hand);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Suspend));
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Wait));
        assert_eq!(hand.add_waiter(exact), Ok(HandAction::Resume));
        assert_eq!(hand.settle_waiter(), HandAction::Wait);
        assert_eq!(hand.phase_succeeded(), Ok(HandAction::Suspend));
        assert_eq!(hand.state, HandState::Suspending);
    }

    proptest! {
        #[test]
        fn arbitrary_waiter_churn_never_changes_the_exact_generation(events in prop::collection::vec(any::<bool>(), 0..512)) {
            let exact = generation(1);
            let mut hand = HandRecord::new(session(), exact, true);
            advance_to_materializing(&mut hand);
            let _ = hand.add_waiter(exact);
            let _ = hand.phase_succeeded();
            for add in events {
                if add {
                    let _ = hand.add_waiter(exact);
                } else {
                    let _ = hand.settle_waiter();
                }
                prop_assert_eq!(hand.generation, Some(exact));
                prop_assert_ne!(hand.state, HandState::Disabled);
            }
        }
    }
}
