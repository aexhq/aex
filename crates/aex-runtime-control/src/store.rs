//! The runtime-activity port.
//!
//! This module is the contract the `aex-runtime-activity-dynamodb` peer implements.
//! It is defined here, in the pure crate, so the lifecycle model never depends on
//! a storage adapter and the adapter has exactly one surface to satisfy.
//!
//! Every method is a plan-in, record-out pair. A plan is a pure value describing a
//! conditional write; the store either lands it and returns the committed record or
//! returns a typed conflict. There is no method that reads, decides and writes,
//! because that shape hides the fence.
//!
//! # Naming
//!
//! Plan 05 sketches the returned intent record as `LifecycleIntent`. That name is
//! taken by `aex_hands_protocol::lifecycle::LifecycleIntent`, which is the
//! *transported action union*, so the durable record here is
//! [`crate::lifecycle::IntentRecord`]. No alias is published: an alias would make
//! a glob import silently resolve to the wrong type, which is precisely the review
//! hazard the rename exists to avoid.

use core::future::Future;
use core::pin::Pin;

use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_hands_protocol::rpc::Fence;
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::meter::Category;
use aex_wire::ids::{GenerationId, OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::generation::{GenerationHead, GenerationState, Revision, TransportMode, supersedes};
use crate::idle::IdleAssessment;
use crate::lifecycle::{
    IntentRecord, IntentState, LifecycleAction, LifecycleIntentId, Lifetime, MicrovmId,
    ProviderState,
};
use crate::usage::SnapshotResidence;

pub use crate::lifecycle::IntentRecord as LifecycleIntentRecord;

/// Which session a generation currently belongs to, and at what fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GenerationPointer {
    /// The session.
    pub session: SessionId,
    /// The current generation.
    pub generation: GenerationId,
    /// The current fence.
    pub fence: Fence,
    /// The current head revision.
    pub revision: Revision,
}

/// What the exact-generation fence decided about an inbound command.
///
/// A lifecycle command names the generation its sender observed. The session
/// authority owns generation identity, so the pointer — never the message — decides
/// whether the command may act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandBinding {
    /// The command names the current generation. It may act under this fence.
    Proceed {
        /// The current fence.
        fence: Fence,
        /// The head revision every conditional write is conditional on.
        revision: Revision,
    },
    /// The session has no generation at all. There is nothing left to act on.
    NoGeneration,
    /// The command names a generation the session has already moved past. Settled:
    /// a redrive would never make it current again.
    Superseded {
        /// What the session points at now.
        current: GenerationId,
    },
    /// The command names a generation strictly *newer* than the pointer. Only the
    /// session authority allocates a generation, so no sender can legitimately know
    /// of one the pointer has never held: the message is poison, not a race.
    Unallocated {
        /// The generation the command named.
        named: GenerationId,
        /// What the session points at.
        current: GenerationId,
    },
}

/// The exact-generation fence, as a pure function of the pointer and the command.
#[must_use]
pub fn bind_command(pointer: Option<&GenerationPointer>, named: GenerationId) -> CommandBinding {
    let Some(pointer) = pointer else {
        return CommandBinding::NoGeneration;
    };
    if pointer.generation == named {
        return CommandBinding::Proceed {
            fence: pointer.fence,
            revision: pointer.revision,
        };
    }
    if supersedes(pointer.generation, named) {
        CommandBinding::Superseded {
            current: pointer.generation,
        }
    } else {
        CommandBinding::Unallocated {
            named,
            current: pointer.generation,
        }
    }
}

/// Everything one lifecycle evaluation needs about a generation, read in one
/// bounded call.
///
/// The head alone is not enough: the worker also has to know which `MicroVM` the
/// generation owns, when the provider started counting its eight hours, whether a
/// lifecycle intent is still open, and which account the resulting usage facts
/// belong to. Reading them as four calls would let the four disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationView {
    /// The immutable exact-generation tuple required to reconstruct the original
    /// launch request. Runtime lifecycle fields below may change; this value may
    /// not. In particular, a launch retry never resolves a newer image catalog.
    pub definition: crate::generation::HandsGeneration,
    /// The head as stored.
    pub head: GenerationHead,
    /// The session the generation belongs to.
    pub session: SessionId,
    /// The workspace the usage happened in.
    pub workspace: WorkspaceId,
    /// The account the money belongs to.
    pub organization: OrganizationId,
    /// The provider `MicroVM`, absent until a launch produces one.
    pub microvm: Option<MicrovmId>,
    /// When the provider started counting the eight-hour lifetime.
    pub lifetime: Option<Lifetime>,
    /// The start of the currently open accounting interval: the launch instant, or
    /// the instant the last receipt closed. A receipt whose `from` were guessed
    /// would either double-charge or drop the gap, and
    /// [`aex_hands_protocol::lifecycle::RuntimeReceipt::validate`] rejects both.
    pub accounted_from: Timestamp,
    /// The most recent lifecycle intent, when one exists. A `dispatched` or
    /// `unknown` intent forbids a second effect.
    pub open_intent: Option<IntentRecord>,
    /// When the current suspension started, for a suspended generation.
    pub suspended_at: Option<Timestamp>,
    /// How many suspensions this generation has had. The AEX-minted snapshot
    /// lifecycle identity counts from zero at launch.
    pub snapshot_ordinal: u32,
    /// The declared retained snapshot size from the signed image catalog.
    pub snapshot_bytes: u64,
}

impl GenerationView {
    /// Whether a new lifecycle effect may be dispatched right now.
    ///
    /// Enforced by the intent's own conditional write as well; this is the cheap
    /// read-side check that keeps the worker from even trying.
    #[must_use]
    pub fn permits_new_effect(&self) -> bool {
        self.open_intent
            .as_ref()
            .is_none_or(IntentRecord::permits_new_effect)
    }
}

/// A conditional write against one generation head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPlan {
    /// The head being written.
    pub generation: GenerationId,
    /// The state the caller read.
    pub expected_state: GenerationState,
    /// The fence the caller read.
    pub expected_fence: Fence,
    /// The revision the caller read.
    pub expected_revision: Revision,
    /// The state to land.
    pub next_state: GenerationState,
    /// The fence to land. It never moves backwards.
    pub next_fence: Fence,
    /// The provider `MicroVM`, once one exists.
    pub microvm: Option<MicrovmId>,
    /// The transport mode, once negotiated.
    pub transport_mode: Option<TransportMode>,
    /// Accounting cursors to land atomically with the state transition.
    /// `None` preserves all three values.
    pub accounting: Option<GenerationAccountingPlan>,
    /// When the write happens.
    pub at: Timestamp,
}

/// Accounting cursors advanced by one settled lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationAccountingPlan {
    /// Start of the next open accounting interval.
    pub accounted_from: Timestamp,
    /// Start of retained snapshot residence, only while suspended.
    pub suspended_at: Option<Timestamp>,
    /// Next snapshot generation ordinal.
    pub snapshot_ordinal: u32,
    /// Provider lifetime start, set exactly once when launch settles. `None`
    /// preserves the already-recorded lifetime on later transitions.
    pub lifetime_started_at: Option<Timestamp>,
}

/// What a committed generation write landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationCommit {
    /// The head as it now stands.
    pub head: GenerationHead,
    /// The revision the write landed.
    pub revision: Revision,
}

/// One conditional increment of the authoritative open-operation count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAdmissionPlan {
    /// The exact generation admitting work.
    pub generation: GenerationId,
    /// The deterministic operation identity. Admission is idempotent on this
    /// value, not merely on a head revision.
    pub operation: aex_hands_protocol::rpc::HandsOperationId,
    /// The lifecycle state the caller observed.
    pub expected_state: GenerationState,
    /// The lifecycle fence presented by the caller.
    pub expected_fence: Fence,
    /// The head revision Brain read.
    pub expected_revision: Revision,
    /// The count after admission.
    pub open_operations: u32,
    /// The revision after admission.
    pub next_revision: Revision,
    /// The lifecycle state to land with admission.
    pub next_state: GenerationState,
    /// The lifecycle fence to land with admission.
    pub next_fence: Fence,
    /// Native-resume evidence to record in the same transaction. Absent for an
    /// ordinary already-running admission.
    pub native_resume: Option<NativeResumeAdmissionPlan>,
    /// The authoritative busy instant.
    pub last_busy_at: Timestamp,
}

/// Evidence recorded atomically when authenticated endpoint traffic is elected
/// to wake one provider-suspended generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResumeAdmissionPlan {
    /// Deterministic lifecycle evidence identity.
    pub intent_id: LifecycleIntentId,
    /// The exact provider `MicroVM` the native policy will wake.
    pub microvm: MicrovmId,
    /// The start of provider-native suspended residence. For a lazily observed
    /// suspension this is derived from the last authoritative guest traffic and
    /// the immutable 180-second provider idle threshold.
    pub suspended_at: Timestamp,
}

/// One conditional decrement of the authoritative open-operation count.
///
/// Settlement is revision-conditional but deliberately not fence-conditional:
/// lifecycle may advance the fence while an admitted operation is completing.
/// A revision race reloads and recomputes rather than losing a decrement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationSettlementPlan {
    /// The exact generation settling work.
    pub generation: GenerationId,
    /// The operation whose durable admission marker is removed. Repeated
    /// settlement of the same operation is a no-op.
    pub operation: aex_hands_protocol::rpc::HandsOperationId,
    /// The head revision Brain read.
    pub expected_revision: Revision,
    /// The count after settlement.
    pub open_operations: u32,
    /// The revision after settlement.
    pub next_revision: Revision,
    /// The authoritative busy instant.
    pub last_busy_at: Timestamp,
}

/// A lifecycle intent to record **before** the provider call, in the same
/// conditional write that takes the fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleIntentPlan {
    /// The intent identity the caller minted.
    pub intent_id: LifecycleIntentId,
    /// The generation being acted on.
    pub generation: GenerationId,
    /// The provider `MicroVM`, absent before a launch produces one.
    pub microvm: Option<MicrovmId>,
    /// What is being attempted.
    pub action: LifecycleAction,
    /// The fence the intent takes.
    pub fence: Fence,
    /// The head state the caller read before taking the lifecycle fence.
    pub expected_state: GenerationState,
    /// The head fence the caller read before taking the lifecycle fence.
    pub expected_fence: Fence,
    /// The head revision the caller read.
    pub expected_revision: Revision,
    /// The transitional state that lands with the intent.
    pub next_state: GenerationState,
    /// When the intent is recorded.
    pub dispatched_at: Timestamp,
}

/// The atomic result of taking a lifecycle fence and recording its intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleIntentCommit {
    /// The head after the fence transition.
    pub generation: GenerationCommit,
    /// The intent that now blocks every second effect.
    pub intent: IntentRecord,
}

/// Provider request evidence to attach before waiting for a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRequestPlan {
    /// Which intent received the provider answer.
    pub intent_id: LifecycleIntentId,
    /// The generation the intent belongs to.
    pub generation: GenerationId,
    /// The exact provider `MicroVM` the request acted on or produced.
    pub microvm: MicrovmId,
    /// The exact request identity returned by the provider SDK.
    pub provider_request_id: ProviderRequestId,
}

/// One durable reconciliation attempt against an unresolved lifecycle intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleReconcilePlan {
    /// Which intent was probed.
    pub intent_id: LifecycleIntentId,
    /// The generation the intent belongs to.
    pub generation: GenerationId,
    /// Attempt count the caller read.
    pub expected_attempts: u32,
    /// The next instant at which the due index may return this generation.
    pub next_evaluate_at: Timestamp,
    /// When the provider probe completed.
    pub reconciled_at: Timestamp,
}

/// The settled outcome of one lifecycle intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleReceiptPlan {
    /// Which intent settles.
    pub intent_id: LifecycleIntentId,
    /// The generation.
    pub generation: GenerationId,
    /// The state the intent settles into.
    pub next_intent_state: IntentState,
    /// The provider request id, which is the only evidence an empty-bodied
    /// lifecycle response carries.
    pub provider_request_id: Option<ProviderRequestId>,
    /// The provider state observed at settlement.
    pub observed_state: Option<ProviderState>,
    /// The snapshot residence this settlement opened or closed.
    pub snapshot: Option<SnapshotResidence>,
    /// Usage drafts written to the transactional outbox with the receipt.
    pub usage: Vec<UsageOutboxPlan>,
    /// Final head/accounting transition committed with the receipt and outbox.
    ///
    /// `None` is reserved for an indeterminate provider outcome whose open intent
    /// stays in `unknown` for reconciliation.
    pub generation_commit: Option<GenerationPlan>,
    /// When the settlement happened.
    pub settled_at: Timestamp,
}

/// One usage draft to persist with a lifecycle settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageOutboxPlan {
    /// Authority ingress the draft belongs to.
    pub category: Category,
    /// Canonical untrusted draft.
    pub draft: FactDraft,
}

/// One durable usage draft awaiting delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageOutboxEntry {
    /// Generation whose lifecycle interval produced it.
    pub generation: GenerationId,
    /// Authority ingress the draft belongs to.
    pub category: Category,
    /// Canonical untrusted draft.
    pub draft: FactDraft,
}

/// A settled lifecycle intent as the store recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleReceipt {
    /// The intent as it now stands.
    pub intent: IntentRecord,
    /// The receipt identity, `lambda-microvm:{microvm}:{action}:{request_id}`.
    pub receipt_id: String,
    /// The snapshot residence, when the settlement closed one.
    pub snapshot: Option<SnapshotResidence>,
}

/// One recorded idle evaluation.
///
/// Writing the probe rearms `next_evaluate_at` in the sparse due index, so the
/// reaper does a bounded ordered scan at zero write cost between evaluations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleProbe {
    /// The generation evaluated.
    pub generation: GenerationId,
    /// What the evaluation saw.
    pub assessment: IdleAssessment,
    /// When the reaper should look again. Always an instant: a generation that
    /// holds provider compute never leaves the due index, because the index is
    /// also what enforces the lifetime and re-examines a stale busy count.
    pub next_evaluate_at: Timestamp,
    /// The head revision the caller read.
    pub expected_revision: Revision,
}

/// One repair of the head's cached open-operation count against the authority.
///
/// Issued when the pre-lock recount proves the counter stale: a leaked count
/// would otherwise hold the generation `Busy` until the provider's hard stop.
/// The write is revision-conditional and rearms the due index in the same
/// update, so a raced repair loses cleanly and a repaired generation stays
/// scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCountRepairPlan {
    /// The generation whose counter is repaired.
    pub generation: GenerationId,
    /// The head revision the caller read.
    pub expected_revision: Revision,
    /// The authoritative open count.
    pub open_operations: u32,
    /// When the reaper should look again.
    pub next_evaluate_at: Timestamp,
    /// When the repair happened. A repair to zero starts the idle clock here.
    pub at: Timestamp,
}

/// Which slice of the due index a scan covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeShard(pub u16);

/// How much one due scan may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBudget {
    /// Largest number of items returned.
    pub max_items: u32,
    /// Largest number of provider-side reads spent.
    pub max_reads: u32,
}

/// One page of due generations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDuePage {
    /// The generations whose evaluation time has arrived.
    pub due: Vec<GenerationPointer>,
    /// The cursor for the next page, absent when the shard is exhausted.
    pub cursor: Option<String>,
}

/// Why a runtime-activity write or read failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeStoreError {
    /// The conditional write lost: the head moved between read and write.
    #[error("the generation head moved: expected revision {expected:?}, found {found:?}")]
    RevisionConflict {
        /// The revision the caller read.
        expected: Revision,
        /// The revision the store holds.
        found: Option<Revision>,
    },
    /// Another reconciler advanced the same open intent first.
    #[error("the lifecycle reconciliation attempt moved: expected {expected}, found {found}")]
    ReconcileConflict {
        /// Attempt count the caller read.
        expected: u32,
        /// Attempt count now stored.
        found: u32,
    },
    /// A second lifecycle effect was attempted while an intent is open.
    #[error("intent `{intent_id}` is still {state:?}; no second effect may be dispatched")]
    IntentOpen {
        /// The open intent.
        intent_id: LifecycleIntentId,
        /// Its state.
        state: IntentState,
    },
    /// The named generation has no head.
    #[error("no head exists for generation {generation}")]
    NoSuchGeneration {
        /// The generation.
        generation: GenerationId,
    },
    /// The store was unavailable; the caller redrives.
    #[error("the runtime-activity store is unavailable: {reason}")]
    Unavailable {
        /// Why.
        reason: String,
    },
    /// The stored item did not decode. Never repaired in place.
    #[error("a runtime-activity item is malformed: {reason}")]
    Malformed {
        /// Why.
        reason: String,
    },
}

/// A boxed store future.
///
/// Public because the port is implemented outside this crate: an adapter must be
/// able to name the return type without restating it.
pub type StoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RuntimeStoreError>> + Send + 'a>>;

/// How a generation view read is served.
///
/// Every caller chooses explicitly. A decision path — admission, a lifecycle
/// transition, a launch — reads strongly, because acting on a stale head is a
/// double effect. A poll path reads eventually: the guest's own generation and
/// fence check already rejects a stale frame, and a strongly consistent
/// `GetItem` on every status poll doubles the read cost of the hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadConsistency {
    /// Linearizable with the last committed write.
    Strong,
    /// Possibly one replication step behind.
    Eventual,
}

/// The runtime-activity port.
///
/// Boxed futures rather than `async fn` so the trait stays object-safe: the worker
/// holds one `Arc<dyn RuntimeActivityStore>`.
pub trait RuntimeActivityStore: Send + Sync + 'static {
    /// The session's current generation pointer, if it has one.
    fn load_current_generation(
        &self,
        session: SessionId,
    ) -> StoreFuture<'_, Option<GenerationPointer>>;

    /// Everything one evaluation needs about a generation, read together.
    fn load_generation_view(
        &self,
        generation: GenerationId,
        consistency: ReadConsistency,
    ) -> StoreFuture<'_, Option<GenerationView>>;

    /// Lands a conditional generation-head write.
    fn commit_generation<'a>(
        &'a self,
        plan: &'a GenerationPlan,
    ) -> StoreFuture<'a, GenerationCommit>;

    /// Atomically admits one Hands operation against running state, exact fence,
    /// and exact revision, advancing the session pointer revision with the head.
    fn admit_operation<'a>(&'a self, plan: &'a OperationAdmissionPlan) -> StoreFuture<'a, ()>;

    /// Atomically settles one Hands operation against only the exact revision,
    /// advancing the session pointer revision with the head.
    fn settle_operation<'a>(&'a self, plan: &'a OperationSettlementPlan) -> StoreFuture<'a, ()>;

    /// Records a lifecycle intent before the provider call.
    fn record_intent<'a>(
        &'a self,
        plan: &'a LifecycleIntentPlan,
    ) -> StoreFuture<'a, LifecycleIntentCommit>;

    /// Persists provider request evidence before waiting for the transition.
    fn record_provider_request<'a>(
        &'a self,
        plan: &'a LifecycleRequestPlan,
    ) -> StoreFuture<'a, IntentRecord>;

    /// Advances an unresolved intent's bounded reconciliation budget.
    fn record_reconcile_attempt<'a>(
        &'a self,
        plan: &'a LifecycleReconcilePlan,
    ) -> StoreFuture<'a, IntentRecord>;

    /// Settles a lifecycle intent with its provider evidence.
    fn settle_intent<'a>(
        &'a self,
        plan: &'a LifecycleReceiptPlan,
    ) -> StoreFuture<'a, LifecycleReceipt>;

    /// Strongly reads all undelivered usage drafts for one generation.
    fn load_usage_outbox(&self, generation: GenerationId)
    -> StoreFuture<'_, Vec<UsageOutboxEntry>>;

    /// Removes one draft only after its authority ingress accepted the enqueue.
    fn mark_usage_emitted<'a>(
        &'a self,
        generation: GenerationId,
        draft: &'a FactDraft,
    ) -> StoreFuture<'a, ()>;

    /// Records one idle evaluation and rearms the due index.
    fn record_probe<'a>(&'a self, probe: &'a IdleProbe) -> StoreFuture<'a, ()>;

    /// Repairs the head's cached open-operation count to the authoritative
    /// value, rearming the due index in the same conditional write.
    fn repair_open_operations<'a>(&'a self, plan: &'a OpenCountRepairPlan) -> StoreFuture<'a, ()>;

    /// Reads one bounded page of due generations.
    fn scan_due(
        &self,
        shard: RuntimeShard,
        now: Timestamp,
        budget: PageBudget,
    ) -> StoreFuture<'_, RuntimeDuePage>;
}

/// The authoritative open-Hands-effect count, read from `session-authority`.
///
/// `openOperations` on the generation head is a fence and a fast path; the
/// authority is Brain's own open Hands effects. The suspend transition recounts
/// against this port **after** taking the lock and **before** any provider call,
/// because acting on a counter that was just proven wrong is how a running job
/// gets snapshotted.
///
/// The production implementation is the bounded strongly-consistent query in
/// `aex-session-dynamodb`. It is a separate port from
/// [`RuntimeActivityStore`] on purpose: the two read different tables, and
/// folding them into one trait would let a control-plane adapter silently acquire
/// a session-authority read.
pub trait OpenEffectCounter: Send + Sync + 'static {
    /// How many Hands effects the session authority currently holds open for this
    /// generation.
    fn count_open_hands_effects(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> StoreFuture<'_, u32>;
}

#[cfg(test)]
mod tests {
    use super::{CommandBinding, GenerationPointer, bind_command};
    use crate::generation::Revision;
    use aex_hands_protocol::rpc::Fence;
    use aex_wire::ids::{GenerationId, PrefixedId as _, SessionId, Uuid7};

    fn generation(millis: u64) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(millis, [1; 10]))
    }

    fn pointer(current: GenerationId) -> GenerationPointer {
        GenerationPointer {
            session: SessionId::from_uuid7(Uuid7::compose(1, [2; 10])),
            generation: current,
            fence: Fence(4),
            revision: Revision::new(9),
        }
    }

    #[test]
    fn a_command_naming_the_current_generation_proceeds_under_the_pointers_fence() {
        let current = generation(10);
        assert_eq!(
            bind_command(Some(&pointer(current)), current),
            CommandBinding::Proceed {
                fence: Fence(4),
                revision: Revision::new(9)
            },
            "the pointer, never the message, supplies the fence"
        );
    }

    #[test]
    fn a_command_naming_a_superseded_generation_is_settled_not_retried() {
        let old = generation(10);
        let new = generation(20);
        assert_eq!(
            bind_command(Some(&pointer(new)), old),
            CommandBinding::Superseded { current: new },
            "a redrive would never make an old generation current again"
        );
    }

    #[test]
    fn a_command_naming_an_unallocated_generation_is_poison() {
        let current = generation(10);
        let invented = generation(20);
        assert_eq!(
            bind_command(Some(&pointer(current)), invented),
            CommandBinding::Unallocated {
                named: invented,
                current
            },
            "only the session authority allocates a generation"
        );
    }

    #[test]
    fn a_session_with_no_pointer_has_nothing_to_act_on() {
        assert_eq!(
            bind_command(None, generation(10)),
            CommandBinding::NoGeneration
        );
    }
}
