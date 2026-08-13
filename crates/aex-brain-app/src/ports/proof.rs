//! Capability tokens: ownership proof, dispatch permission, cancellation and preview.

use crate::kernel::sync::Arc;
use crate::kernel::sync::atomic::{AtomicBool, Ordering};
use aex_brain_domain::commit::FenceGuardRef;
use aex_brain_domain::ids::{
    AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, JournalSeq, OwnerToken, Timestamp,
};
use aex_model_catalog::canonical::PreviewFrame;
use aex_wire::ids::{OrganizationId, WorkspaceId};

/// Proof that the holder claimed an agent and has not been fenced out.
///
/// Every store write takes one. The guard is not the correctness mechanism — the durable
/// precondition set is — but requiring it means a caller cannot *forget* to condition a
/// write on the fence it holds, because there is no way to name the agent without it.
///
/// A guard is minted only from a [`Claim`](super::store::Claim), which only
/// [`LeaseStore::claim`](super::store::LeaseStore::claim) returns.
#[derive(Debug, Clone)]
pub struct FenceGuard {
    key: AgentKey,
    owner: OwnerToken,
    fence: Fence,
    revision: AgentRevision,
    tail: Option<JournalSeq>,
    cancel_epoch: CancelEpoch,
    cancel: CancelToken,
}

impl FenceGuard {
    /// Builds the guard for a claim the store just granted.
    ///
    /// The store adapter is the only intended caller: everything it needs comes from the
    /// conditional write that made the holder the owner.
    #[must_use]
    pub fn new(
        key: AgentKey,
        owner: OwnerToken,
        fence: Fence,
        revision: AgentRevision,
        tail: Option<JournalSeq>,
        cancel_epoch: CancelEpoch,
        cancel: CancelToken,
    ) -> Self {
        Self {
            key,
            owner,
            fence,
            revision,
            tail,
            cancel_epoch,
            cancel,
        }
    }

    /// Which agent this guard owns.
    #[must_use]
    pub const fn key(&self) -> AgentKey {
        self.key
    }

    /// Which ownership generation the holder took.
    #[must_use]
    pub const fn fence(&self) -> Fence {
        self.fence
    }

    /// The revision every write conditions on.
    #[must_use]
    pub const fn revision(&self) -> AgentRevision {
        self.revision
    }

    /// The journal tail every append is contiguous from.
    #[must_use]
    pub const fn tail(&self) -> Option<JournalSeq> {
        self.tail
    }

    /// The cancellation epoch every write conditions on.
    #[must_use]
    pub const fn cancel_epoch(&self) -> CancelEpoch {
        self.cancel_epoch
    }

    /// The token that is set when the lease is lost or the run is cancelled.
    #[must_use]
    pub fn cancel(&self) -> &CancelToken {
        &self.cancel
    }

    /// The precondition set a [`DecisionCommit`](aex_brain_domain::commit::DecisionCommit)
    /// carries.
    #[must_use]
    pub const fn as_ref(&self) -> FenceGuardRef {
        FenceGuardRef {
            key: self.key,
            owner: self.owner,
            fence: self.fence,
            revision: self.revision,
            tail: self.tail,
            cancel_epoch: self.cancel_epoch,
        }
    }

    /// The guard the next commit under this claim carries.
    ///
    /// Revision and tail advance; the fence does not, because a commit does not change
    /// hands. Those are two separable monotonic facts and conflating them would make a
    /// renewal look like a takeover.
    #[must_use]
    pub fn advanced(&self, next_revision: AgentRevision, next_tail: JournalSeq) -> Self {
        Self {
            revision: next_revision,
            tail: Some(next_tail),
            ..self.clone()
        }
    }
}

/// Permission to let a byte leave the process for one effect attempt.
///
/// Minted only by [`EffectStore::mark_dispatch_started`](super::store::EffectStore::mark_dispatch_started),
/// which is the durable conditional write that happens **before** any dispatch. Because
/// [`ProviderPort::dispatch`](super::provider::ProviderPort::dispatch),
/// [`ToolPort::invoke`](super::tool::ToolPort::invoke) and
/// [`HandsPort::start`](super::hands::HandsPort::start) all require one, dispatching
/// without that durable write is a compile error rather than a review comment.
///
/// The ticket carries no `Clone`: one durable pre-send write authorizes exactly one
/// dispatch attempt.
#[derive(Debug)]
pub struct DispatchTicket {
    effect: EffectId,
    attempt: u16,
    fence: Fence,
    key: AgentKey,
    at: Timestamp,
    workspace: WorkspaceId,
    organization: OrganizationId,
}

impl DispatchTicket {
    /// Mints the ticket for `effect` under `guard`.
    ///
    /// Requiring the guard is the point: a ticket cannot exist without proof of ownership,
    /// so a losing owner cannot manufacture permission to dispatch.
    #[must_use]
    pub const fn mint(
        guard: &FenceGuard,
        workspace: WorkspaceId,
        organization: OrganizationId,
        effect: EffectId,
        attempt: u16,
        at: Timestamp,
    ) -> Self {
        Self {
            effect,
            attempt,
            fence: guard.fence,
            key: guard.key,
            at,
            workspace,
            organization,
        }
    }

    /// Which effect this ticket authorizes.
    #[must_use]
    pub const fn effect(&self) -> EffectId {
        self.effect
    }

    /// Which attempt.
    #[must_use]
    pub const fn attempt(&self) -> u16 {
        self.attempt
    }

    /// The fence the ticket was minted under.
    #[must_use]
    pub const fn fence(&self) -> Fence {
        self.fence
    }

    /// Which agent.
    #[must_use]
    pub const fn key(&self) -> AgentKey {
        self.key
    }

    /// When the durable pre-send write committed.
    #[must_use]
    pub const fn issued_at(&self) -> Timestamp {
        self.at
    }

    /// The workspace whose credentials and transport pools this dispatch may use.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        self.workspace
    }

    /// The organization whose KMS encryption context this dispatch may use.
    #[must_use]
    pub const fn organization(&self) -> OrganizationId {
        self.organization
    }

    /// Checks that this ticket belongs to `effect`.
    ///
    /// # Errors
    ///
    /// Returns [`TicketMismatch`] when it does not. An adapter handed the wrong ticket must
    /// fail loudly rather than dispatch the effect the ticket actually names.
    pub fn expect(&self, effect: EffectId) -> Result<(), TicketMismatch> {
        if self.effect == effect {
            Ok(())
        } else {
            Err(TicketMismatch {
                ticket: self.effect,
                requested: effect,
            })
        }
    }
}

/// A ticket was presented for an effect it does not authorize.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ticket authorizes effect {ticket} but {requested} was requested")]
pub struct TicketMismatch {
    /// What the ticket authorizes.
    pub ticket: EffectId,
    /// What the caller asked to dispatch.
    pub requested: EffectId,
}

/// A one-way flag: once set, it stays set.
///
/// Set by a lost lease, an observed fence advance, a cancellation epoch advance or drain.
/// Adapters poll it to abandon work early; it is a courtesy, never the correctness
/// mechanism. A cancelled activation still settles its dispatched effects from durable
/// evidence, because a token in this process cannot un-send a request.
#[derive(Debug, Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl CancelToken {
    /// A token that has not been set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the token. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether the token has been set.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Where preview frames go.
///
/// Preview is observation, never authority: a frame that never arrives changes no durable
/// fact, and [`PreviewFrame`] has no conversion into any journal variant, so a partial
/// stream is structurally unable to become model-visible history.
pub trait PreviewSink: Send + Sync {
    /// Offers one frame. Implementations coalesce and may drop under pressure; the return
    /// value says whether the frame was accepted, and a caller must not treat `false` as an
    /// error worth failing the effect over.
    fn offer(&self, frame: PreviewFrame) -> bool;
}

/// A sink that accepts nothing.
///
/// Used where a caller genuinely has no client attached. It is a real type rather than an
/// `Option<&dyn PreviewSink>` so an adapter has exactly one code path.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullPreviewSink;

impl PreviewSink for NullPreviewSink {
    fn offer(&self, _frame: PreviewFrame) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelToken, DispatchTicket, FenceGuard, NullPreviewSink, PreviewSink};
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, JournalSeq, OwnerToken,
        SessionId, Timestamp,
    };
    use aex_brain_domain::wire_pending::PreviewFrame;
    use aex_model_catalog::BoundedString;
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use uuid::Uuid;

    fn guard() -> FenceGuard {
        FenceGuard::new(
            AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
            OwnerToken(Uuid::from_u128(3)),
            Fence(4),
            AgentRevision(5),
            Some(JournalSeq(6)),
            CancelEpoch(7),
            CancelToken::new(),
        )
    }

    #[test]
    fn a_guard_projects_the_whole_precondition_set() {
        let guard = guard();
        let reference = guard.as_ref();
        assert_eq!(reference.fence, Fence(4));
        assert_eq!(reference.revision, AgentRevision(5));
        assert_eq!(reference.tail, Some(JournalSeq(6)));
        assert_eq!(reference.cancel_epoch, CancelEpoch(7));
    }

    #[test]
    fn advancing_a_guard_moves_the_revision_but_never_the_fence() {
        let guard = guard();
        let next = guard.advanced(AgentRevision(6), JournalSeq(9));
        assert_eq!(
            next.fence(),
            guard.fence(),
            "a commit does not change hands"
        );
        assert_eq!(next.revision(), AgentRevision(6));
        assert_eq!(next.tail(), Some(JournalSeq(9)));
    }

    #[test]
    fn a_ticket_carries_the_fence_it_was_minted_under() {
        let guard = guard();
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10]));
        let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [9; 10]));
        let ticket = DispatchTicket::mint(
            &guard,
            workspace,
            organization,
            EffectId([1; 16]),
            2,
            Timestamp(10),
        );
        assert_eq!(ticket.fence(), Fence(4));
        assert_eq!(ticket.attempt(), 2);
        assert_eq!(ticket.key(), guard.key());
        assert_eq!(ticket.issued_at(), Timestamp(10));
        assert_eq!(ticket.workspace(), workspace);
        assert_eq!(ticket.organization(), organization);
    }

    #[test]
    fn a_ticket_refuses_an_effect_it_does_not_authorize() {
        let ticket = DispatchTicket::mint(
            &guard(),
            WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10])),
            OrganizationId::from_uuid7(Uuid7::compose(1, [9; 10])),
            EffectId([1; 16]),
            1,
            Timestamp(0),
        );
        ticket.expect(EffectId([1; 16])).expect("its own effect");
        let error = ticket
            .expect(EffectId([2; 16]))
            .expect_err("a different effect is refused");
        assert_eq!(error.ticket, EffectId([1; 16]));
        assert_eq!(error.requested, EffectId([2; 16]));
    }

    #[test]
    fn a_cancel_token_is_one_way_and_shared() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled(), "the flag is shared, not copied");
        clone.cancel();
        assert!(token.is_cancelled(), "setting twice is idempotent");
    }

    #[test]
    fn the_null_sink_accepts_nothing_and_says_so() {
        let accepted = NullPreviewSink.offer(PreviewFrame::TextDelta {
            index: 0,
            text: BoundedString::truncating("delta"),
        });
        assert!(!accepted);
    }
}
