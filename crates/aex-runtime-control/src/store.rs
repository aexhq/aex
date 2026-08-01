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
use aex_wire::ids::{GenerationId, SessionId};
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::generation::{GenerationHead, GenerationState, Revision, TransportMode};
use crate::idle::IdleAssessment;
use crate::lifecycle::{
    IntentRecord, IntentState, LifecycleAction, LifecycleIntentId, MicrovmId, ProviderState,
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

/// A conditional write against one generation head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPlan {
    /// The head being written.
    pub generation: GenerationId,
    /// The state the caller read.
    pub expected_state: GenerationState,
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
    /// When the write happens.
    pub at: Timestamp,
}

/// What a committed generation write landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationCommit {
    /// The head as it now stands.
    pub head: GenerationHead,
    /// The revision the write landed.
    pub revision: Revision,
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
    /// The head revision the caller read.
    pub expected_revision: Revision,
    /// When the intent is recorded.
    pub dispatched_at: Timestamp,
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
    /// When the settlement happened.
    pub settled_at: Timestamp,
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
    /// When the reaper should look again.
    pub next_evaluate_at: Option<Timestamp>,
    /// The head revision the caller read.
    pub expected_revision: Revision,
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
type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RuntimeStoreError>> + Send + 'a>>;

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

    /// Lands a conditional generation-head write.
    fn commit_generation<'a>(
        &'a self,
        plan: &'a GenerationPlan,
    ) -> StoreFuture<'a, GenerationCommit>;

    /// Records a lifecycle intent before the provider call.
    fn record_intent<'a>(&'a self, plan: &'a LifecycleIntentPlan) -> StoreFuture<'a, IntentRecord>;

    /// Settles a lifecycle intent with its provider evidence.
    fn settle_intent<'a>(
        &'a self,
        plan: &'a LifecycleReceiptPlan,
    ) -> StoreFuture<'a, LifecycleReceipt>;

    /// Records one idle evaluation and rearms the due index.
    fn record_probe<'a>(&'a self, probe: &'a IdleProbe) -> StoreFuture<'a, ()>;

    /// Reads one bounded page of due generations.
    fn scan_due(
        &self,
        shard: RuntimeShard,
        now: Timestamp,
        budget: PageBudget,
    ) -> StoreFuture<'_, RuntimeDuePage>;
}
