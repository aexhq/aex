//! The public session lifecycle over one retained provider generation.
//!
//! This state machine deliberately exposes no run or turn resource. An
//! internal run identity is carried only while Brain owns the current message;
//! every completion or cancellation returns the session to `Idle`, ready for a
//! later message. Provider-native idle suspension and maximum-duration policy
//! own automatic timing; API activity reconciles observed provider state. No
//! periodic scan or platform timer is part of the product.

use aex_internal_contracts::RunId;
use aex_wire::ids::{GenerationId, MessageId};
use aex_wire::types::Timestamp;

use crate::ResolvedMessageBounds;

/// An idle generation is suspended after this exact interval.
pub const IDLE_SUSPEND_AFTER_SECONDS: i64 = 180;
/// A provider generation may live for at most eight hours.
pub const MAXIMUM_LIFETIME_SECONDS: i64 = 28_800;

const MILLIS_PER_SECOND: i64 = 1_000;
const IDLE_SUSPEND_AFTER_MILLIS: i64 = IDLE_SUSPEND_AFTER_SECONDS * MILLIS_PER_SECOND;
const MAXIMUM_LIFETIME_MILLIS: i64 = MAXIMUM_LIFETIME_SECONDS * MILLIS_PER_SECOND;

/// What the session is doing, independent of an internal run's own fold state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecycleStatus {
    /// Ready for another message.
    Idle,
    /// Processing the active message.
    Running,
    /// Stopping compute while retaining the exact generation.
    Suspending,
    /// Compute is stopped and the exact generation may resume.
    Suspended,
    /// Restarting the retained generation.
    Resuming,
    /// Permanently destroying compute and live files.
    Terminating,
    /// Compute and live files are permanently gone.
    Terminated,
    /// Irreversible metadata and telemetry deletion is in progress.
    Deleting,
}

impl LifecycleStatus {
    /// Every public state in canonical order.
    pub const ALL: [Self; 8] = [
        Self::Idle,
        Self::Running,
        Self::Suspending,
        Self::Suspended,
        Self::Resuming,
        Self::Terminating,
        Self::Terminated,
        Self::Deleting,
    ];

    /// Whether Brain currently owns a message.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Why the retained generation was permanently destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TerminationReason {
    /// The customer explicitly terminated it.
    User,
    /// Its eight-hour lifetime elapsed.
    LifetimeExpired,
    /// The pinned provider credential was revoked.
    ProviderCredentialRevoked,
    /// The runtime was lost; crash recovery is not part of this release.
    RuntimeLost,
}

/// The one message Brain currently owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveMessage {
    /// The public message identity.
    pub message: MessageId,
    /// Internal execution identity, never projected to public wire.
    pub run: RunId,
    /// The run-local spend cap; this is not an account reservation.
    pub bounds: ResolvedMessageBounds,
}

/// Session-centric lifecycle authority for one immutable generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLifecycle {
    /// The one immutable provider generation this lifecycle controls.
    pub generation: Option<GenerationId>,
    /// Monotonic token that invalidates every older delayed event.
    pub revision: LifecycleRevision,
    /// Current public state.
    pub status: LifecycleStatus,
    /// Current message activity, only while running.
    pub active: Option<ActiveMessage>,
    /// When the provider generation first launched.
    pub launched_at: Timestamp,
    /// Exactly eight hours after `launched_at`.
    pub expires_at: Timestamp,
    /// When the session most recently became idle.
    pub idle_since: Option<Timestamp>,
    /// Exact provider-native idle-suspension threshold for the current idle interval.
    pub suspend_at: Option<Timestamp>,
    /// When this generation most recently suspended.
    pub suspended_at: Option<Timestamp>,
    /// When this generation permanently terminated.
    pub terminated_at: Option<Timestamp>,
    /// Why it terminated.
    pub termination_reason: Option<TerminationReason>,
}

/// Monotonic lifecycle concurrency token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LifecycleRevision(pub u64);

/// Why a session lifecycle transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionLifecycleError {
    /// The launch instant cannot express its eight-hour expiry.
    #[error("the session launch instant cannot express its lifecycle deadlines")]
    TimestampOverflow,
    /// No more lifecycle revisions can be represented.
    #[error("the session lifecycle revision is exhausted")]
    RevisionExhausted,
    /// The current state cannot admit that transition.
    #[error("session lifecycle state {0:?} does not admit this transition")]
    InvalidState(LifecycleStatus),
    /// Active work must be cancelled before suspension.
    #[error("the session is active; cancel current work before suspending")]
    SessionBusy,
    /// The immutable provider lifetime has elapsed.
    #[error("the session provider lifetime has elapsed")]
    LifetimeExpired,
    /// Completion named a different internal owner.
    #[error("the current message belongs to another internal execution")]
    ActivityMismatch,
    /// Compute and live files must be destroyed before user-content deletion.
    #[error("the session must terminate before irreversible deletion begins")]
    TerminationRequired,
}

pub(crate) type LifecycleError = SessionLifecycleError;

impl SessionLifecycle {
    /// Starts an idle, launched generation with exact suspend and expiry rows.
    ///
    /// # Errors
    ///
    /// Returns [`LifecycleError::TimestampOverflow`] near the end of the wire
    /// timestamp range.
    pub fn launched(
        generation: GenerationId,
        launched_at: Timestamp,
    ) -> Result<Self, LifecycleError> {
        let expires_at = add_millis(launched_at, MAXIMUM_LIFETIME_MILLIS)?;
        let suspend_at = add_millis(launched_at, IDLE_SUSPEND_AFTER_MILLIS)?;
        Ok(Self {
            generation: Some(generation),
            revision: LifecycleRevision(0),
            status: LifecycleStatus::Idle,
            active: None,
            launched_at,
            expires_at,
            idle_since: Some(launched_at),
            suspend_at: Some(suspend_at),
            suspended_at: None,
            terminated_at: None,
            termination_reason: None,
        })
    }

    /// Starts the same bounded session lifecycle without allocating a runtime.
    /// Message admission remains available, but sandbox lifecycle commands have
    /// no generation to target.
    ///
    /// # Errors
    ///
    /// Returns [`LifecycleError::TimestampOverflow`] near the end of the wire
    /// timestamp range.
    pub fn sandbox_disabled(created_at: Timestamp) -> Result<Self, LifecycleError> {
        let expires_at = add_millis(created_at, MAXIMUM_LIFETIME_MILLIS)?;
        Ok(Self {
            generation: None,
            revision: LifecycleRevision(0),
            status: LifecycleStatus::Idle,
            active: None,
            launched_at: created_at,
            expires_at,
            idle_since: Some(created_at),
            suspend_at: None,
            suspended_at: None,
            terminated_at: None,
            termination_reason: None,
        })
    }

    /// Admits one message, automatically resuming the same suspended generation.
    ///
    /// # Errors
    ///
    /// Refuses an elapsed lifetime or a state other than idle/suspended.
    pub fn admit_message(
        &mut self,
        message: MessageId,
        run: RunId,
        bounds: ResolvedMessageBounds,
        now: Timestamp,
    ) -> Result<(), LifecycleError> {
        if now >= self.expires_at || bounds.deadline > self.expires_at {
            return Err(LifecycleError::LifetimeExpired);
        }
        if !matches!(
            self.status,
            LifecycleStatus::Idle | LifecycleStatus::Suspended
        ) {
            return Err(if self.status.is_active() {
                LifecycleError::SessionBusy
            } else {
                LifecycleError::InvalidState(self.status)
            });
        }
        self.bump_revision()?;
        self.status = if self.status == LifecycleStatus::Suspended {
            LifecycleStatus::Resuming
        } else {
            LifecycleStatus::Running
        };
        self.active = Some(ActiveMessage {
            message,
            run,
            bounds,
        });
        self.idle_since = None;
        self.suspend_at = None;
        Ok(())
    }

    /// Completes the active message and returns the session to idle.
    ///
    /// # Errors
    ///
    /// Refuses an inactive lifecycle, a different active owner, an elapsed
    /// lifetime, an unrepresentable suspend deadline, or an exhausted revision.
    pub fn complete_message(&mut self, run: RunId, now: Timestamp) -> Result<(), LifecycleError> {
        if !self.status.is_active() {
            return Err(LifecycleError::InvalidState(self.status));
        }
        ensure_owner(self.active, run)?;
        self.become_idle(now)
    }

    /// Cancels current work. An already-idle/suspended session is a no-op.
    ///
    /// # Errors
    ///
    /// Refuses a terminal transition state, an elapsed lifetime, an
    /// unrepresentable suspend deadline, or an exhausted lifecycle revision.
    pub fn cancel_current(&mut self, now: Timestamp) -> Result<bool, LifecycleError> {
        if self.status.is_active() {
            self.become_idle(now)?;
            return Ok(true);
        }
        if matches!(
            self.status,
            LifecycleStatus::Idle | LifecycleStatus::Suspended
        ) {
            return Ok(false);
        }
        Err(LifecycleError::InvalidState(self.status))
    }

    /// Begins manual or automatic suspension. Active work is never paused.
    ///
    /// # Errors
    ///
    /// Refuses active work, any other invalid lifecycle state, or an exhausted
    /// lifecycle revision.
    pub fn begin_suspend(&mut self) -> Result<bool, LifecycleError> {
        match self.status {
            LifecycleStatus::Idle => {
                self.bump_revision()?;
                self.status = LifecycleStatus::Suspending;
                self.suspend_at = None;
                Ok(true)
            }
            LifecycleStatus::Suspended => Ok(false),
            LifecycleStatus::Running => Err(LifecycleError::SessionBusy),
            state => Err(LifecycleError::InvalidState(state)),
        }
    }

    /// Records that compute stopped while retaining this exact generation.
    ///
    /// # Errors
    ///
    /// Refuses a lifecycle that is not suspending or an exhausted lifecycle
    /// revision.
    pub fn complete_suspend(&mut self, now: Timestamp) -> Result<(), LifecycleError> {
        if self.status != LifecycleStatus::Suspending {
            return Err(LifecycleError::InvalidState(self.status));
        }
        self.bump_revision()?;
        self.status = LifecycleStatus::Suspended;
        self.suspended_at = Some(now);
        self.idle_since = None;
        self.suspend_at = None;
        Ok(())
    }

    /// Begins manual resume of the retained generation.
    ///
    /// # Errors
    ///
    /// Refuses a lifecycle that is neither suspended nor idle, or an exhausted
    /// lifecycle revision.
    pub fn begin_resume(&mut self) -> Result<bool, LifecycleError> {
        match self.status {
            LifecycleStatus::Suspended => {
                self.bump_revision()?;
                self.status = LifecycleStatus::Resuming;
                Ok(true)
            }
            LifecycleStatus::Idle => Ok(false),
            state => Err(LifecycleError::InvalidState(state)),
        }
    }

    /// Records that the retained generation resumed.
    ///
    /// A manual resume becomes idle. An automatic message-triggered resume
    /// starts the already-admitted message only after compute is available.
    ///
    /// # Errors
    ///
    /// Refuses a lifecycle that is not resuming, an elapsed lifetime, an
    /// unrepresentable suspend deadline, or an exhausted lifecycle revision.
    pub fn complete_resume(&mut self, now: Timestamp) -> Result<(), LifecycleError> {
        if self.status != LifecycleStatus::Resuming {
            return Err(LifecycleError::InvalidState(self.status));
        }
        if self.active.is_some() {
            self.bump_revision()?;
            self.status = LifecycleStatus::Running;
            self.suspended_at = None;
            return Ok(());
        }
        self.become_idle(now)
    }

    /// Begins permanent termination and clears any active work.
    ///
    /// # Errors
    ///
    /// Refuses deletion already in progress or an exhausted lifecycle revision.
    pub fn begin_terminate(&mut self, reason: TerminationReason) -> Result<bool, LifecycleError> {
        match self.status {
            LifecycleStatus::Terminating | LifecycleStatus::Terminated => Ok(false),
            LifecycleStatus::Deleting => Err(LifecycleError::InvalidState(self.status)),
            _ => {
                self.bump_revision()?;
                self.status = LifecycleStatus::Terminating;
                self.active = None;
                self.idle_since = None;
                self.suspend_at = None;
                self.termination_reason = Some(reason);
                Ok(true)
            }
        }
    }

    /// Records that compute and live files were permanently destroyed.
    ///
    /// # Errors
    ///
    /// Refuses a lifecycle that is not terminating or an exhausted lifecycle
    /// revision.
    pub fn complete_terminate(&mut self, now: Timestamp) -> Result<(), LifecycleError> {
        if self.status != LifecycleStatus::Terminating {
            return Err(LifecycleError::InvalidState(self.status));
        }
        self.bump_revision()?;
        self.status = LifecycleStatus::Terminated;
        self.terminated_at = Some(now);
        Ok(())
    }

    /// Starts irreversible user-content deletion after termination is proven.
    ///
    /// # Errors
    ///
    /// Refuses a lifecycle that is not terminated or an exhausted lifecycle
    /// revision.
    pub fn begin_delete(&mut self) -> Result<(), LifecycleError> {
        if self.status != LifecycleStatus::Terminated {
            return Err(LifecycleError::TerminationRequired);
        }
        self.bump_revision()?;
        self.status = LifecycleStatus::Deleting;
        Ok(())
    }

    fn become_idle(&mut self, now: Timestamp) -> Result<(), LifecycleError> {
        if now >= self.expires_at {
            return Err(LifecycleError::LifetimeExpired);
        }
        let suspend_at = add_millis(now, IDLE_SUSPEND_AFTER_MILLIS)?;
        self.bump_revision()?;
        self.status = LifecycleStatus::Idle;
        self.active = None;
        self.idle_since = Some(now);
        self.suspend_at = Some(suspend_at);
        self.suspended_at = None;
        Ok(())
    }

    fn bump_revision(&mut self) -> Result<(), LifecycleError> {
        self.revision.0 = self
            .revision
            .0
            .checked_add(1)
            .ok_or(LifecycleError::RevisionExhausted)?;
        Ok(())
    }
}

fn ensure_owner(active: Option<ActiveMessage>, run: RunId) -> Result<(), LifecycleError> {
    match active {
        Some(active) if active.run == run => Ok(()),
        _ => Err(LifecycleError::ActivityMismatch),
    }
}

fn add_millis(value: Timestamp, delta: i64) -> Result<Timestamp, LifecycleError> {
    value
        .unix_millis()
        .checked_add(delta)
        .and_then(|millis| Timestamp::from_unix_millis(millis).ok())
        .ok_or(LifecycleError::TimestampOverflow)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use aex_internal_contracts::RunId;
    use aex_wire::ids::{GenerationId, MessageId, PrefixedId as _, Uuid7};

    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn message(tag: u8) -> MessageId {
        MessageId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn run(tag: u8) -> RunId {
        RunId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn generation(tag: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn bounds(deadline: Timestamp) -> ResolvedMessageBounds {
        ResolvedMessageBounds {
            max_spend_cents: NonZeroU64::new(1_000).expect("positive"),
            deadline,
        }
    }

    #[test]
    fn launch_pins_exact_idle_and_lifetime_deadlines() {
        let lifecycle = SessionLifecycle::launched(generation(1), at(1_000)).expect("launch");
        assert_eq!(lifecycle.suspend_at, Some(at(181_000)));
        assert_eq!(lifecycle.expires_at, at(28_801_000));
    }

    #[test]
    fn messages_are_multi_turn_and_suspended_admission_resumes_the_same_generation() {
        let mut lifecycle = SessionLifecycle::launched(generation(1), at(0)).expect("launch");
        lifecycle
            .admit_message(message(1), run(1), bounds(at(10_000)), at(1_000))
            .expect("first message");
        lifecycle
            .complete_message(run(1), at(2_000))
            .expect("finish");
        assert_eq!(lifecycle.status, LifecycleStatus::Idle);
        assert_eq!(lifecycle.suspend_at, Some(at(182_000)));

        lifecycle.begin_suspend().expect("begin suspend");
        lifecycle.complete_suspend(at(3_000)).expect("suspend");
        lifecycle
            .admit_message(message(2), run(2), bounds(at(20_000)), at(4_000))
            .expect("message auto-resumes");
        assert_eq!(lifecycle.status, LifecycleStatus::Resuming);
        lifecycle
            .complete_resume(at(5_000))
            .expect("generation resumed");
        assert_eq!(lifecycle.status, LifecycleStatus::Running);
        assert_eq!(lifecycle.suspended_at, None);
        assert_eq!(lifecycle.active.expect("activity").message, message(2));
    }

    #[test]
    fn active_work_must_be_cancelled_before_manual_suspend() {
        let mut lifecycle = SessionLifecycle::launched(generation(1), at(0)).expect("launch");
        lifecycle
            .admit_message(message(1), run(1), bounds(at(10_000)), at(1_000))
            .expect("message");
        assert_eq!(lifecycle.begin_suspend(), Err(LifecycleError::SessionBusy));
        assert_eq!(lifecycle.cancel_current(at(2_000)), Ok(true));
        assert_eq!(lifecycle.begin_suspend(), Ok(true));
    }

    #[test]
    fn provider_native_automatic_state_is_reconciled_without_a_platform_timer() {
        let mut lifecycle = SessionLifecycle::launched(generation(1), at(0)).expect("launch");
        assert_eq!(lifecycle.suspend_at, Some(at(180_000)));
        lifecycle
            .begin_suspend()
            .expect("provider reports auto-suspend");
        lifecycle.complete_suspend(at(180_001)).expect("suspended");
        lifecycle
            .begin_terminate(TerminationReason::LifetimeExpired)
            .expect("provider reports native maximum duration");
        lifecycle
            .complete_terminate(at(28_800_000))
            .expect("terminated");
        assert_eq!(
            lifecycle.termination_reason,
            Some(TerminationReason::LifetimeExpired)
        );
    }

    #[test]
    fn delete_cannot_run_before_compute_and_live_files_are_destroyed() {
        let mut lifecycle = SessionLifecycle::launched(generation(1), at(0)).expect("launch");
        assert_eq!(
            lifecycle.begin_delete(),
            Err(LifecycleError::TerminationRequired)
        );
        lifecycle
            .begin_terminate(TerminationReason::User)
            .expect("terminate");
        lifecycle.complete_terminate(at(1_000)).expect("destroyed");
        lifecycle.begin_delete().expect("delete");
        assert_eq!(lifecycle.status, LifecycleStatus::Deleting);
    }

    #[test]
    fn public_activity_does_not_expose_an_internal_run_identity() {
        let mut lifecycle = SessionLifecycle::launched(generation(1), at(0)).expect("launch");
        lifecycle
            .admit_message(message(1), run(1), bounds(at(10_000)), at(1_000))
            .expect("message");
        let public = (
            lifecycle.status,
            lifecycle.active.map(|activity| activity.message),
            lifecycle
                .active
                .map(|activity| activity.bounds.max_spend_cents),
            lifecycle.active.map(|activity| activity.bounds.deadline),
        );
        assert_eq!(public.1, Some(message(1)));
        assert_eq!(public.2.map(NonZeroU64::get), Some(1_000));
        assert_eq!(public.3, Some(at(10_000)));
    }
}
