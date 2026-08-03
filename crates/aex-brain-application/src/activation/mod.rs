//! The activation: one wake, driven from claim to ack.
//!
//! This module is the loop plan 07 §10 deferred. It is deliberately runtime-free — nothing
//! here names Tokio, a timer or a thread — so the composition root chooses the scheduler and
//! this code stays exercisable without one.
//!
//! # The order, and why every part of it is load-bearing
//!
//! ```text
//! receive -> dedupe -> local slot -> claim -> recover -> fold -> plan -> dispatch -> settle -> commit -> release -> ack
//! ```
//!
//! - **Receive never originates work.** [`WakeQueue`](crate::ports::WakeQueue) has no
//!   `enqueue`; a wake exists only inside a [`DecisionCommit`], so the queue is a delivery
//!   hint and the durable row is the fact. Nothing in this module creates a delivery.
//! - **The local slot is not the correctness mechanism.**
//!   [`ActivationRegistry`](crate::kernel::ActivationRegistry) removes duplicate *local*
//!   work; the durable fence decides who may write.
//! - **Recovery precedes planning.** An effect left open by a dead owner is classified by
//!   [`recover`](aex_brain_domain::effect::recover) before the planner is consulted, because
//!   the planner has no dispatch evidence and must not guess. A possibly-received provider
//!   request settles `OutcomeUnknown`; it never becomes a second generation.
//! - **The ack is strictly last.** Deleting a message is not a commit. A crash between the
//!   commit and the ack redelivers the wake, and the redelivered activation folds the
//!   journal the commit already wrote, so it plans the *next* step rather than repeating the
//!   last one.
//! - **A refusal releases rather than acks.** Every path that did not commit puts the
//!   delivery back, so overload, a lost race and a store fault all cost a redelivery instead
//!   of a lost unit of work.

pub mod decide;
pub mod run;

#[cfg(any(test, feature = "testing"))]
pub mod memory;

#[cfg(test)]
mod tests;

use crate::ports::{
    CatalogError, CatalogPort, ClaimError, ClockPort, CommitError, EffectStore, HandsError,
    HandsPort, IdPort, JournalStore, LeaseStore, ProviderPort, ReadBudget, StoreError, ToolPort,
    ToolRoutingError, WakeQueue,
};
use aex_brain_domain::context::ContextPolicy;
use aex_brain_domain::fold::FoldError;
use aex_brain_domain::ids::WorkShard;
use aex_brain_domain::journal::FinishReason;
use std::sync::Arc;

pub use decide::{Draft, phase_tag};
pub use run::{Activation, PollReport, WakeLoop};

/// The strict activation-wide ceiling for cold journal restore.
///
/// This is separate from [`ReadBudget`]: a service may return many individually valid short
/// pages, and retaining all of them is still unbounded unless the activation accounts for
/// the whole restore. It is an entry/byte ceiling only; model token limits are not a memory
/// measurement and are deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreBudget {
    /// The most entries one activation may retain while rebuilding its fold.
    pub max_entries: usize,
    /// The most hydrated journal bytes one activation may retain while rebuilding its fold.
    pub max_bytes: usize,
}

/// Every port one activation is driven through.
///
/// One value rather than ten parameters: the ports have to agree about the agent they are
/// addressing, and a bundle that is constructed once cannot be assembled inconsistently at a
/// call site.
#[derive(Clone)]
pub struct Ports {
    /// The agent's journal: head, pages and the one decision transaction.
    pub journal: Arc<dyn JournalStore>,
    /// The durable effect record's two pre-settlement transitions.
    pub effects: Arc<dyn EffectStore>,
    /// Activation ownership.
    pub leases: Arc<dyn LeaseStore>,
    /// Wake delivery. Never wake creation.
    pub wakes: Arc<dyn WakeQueue>,
    /// Model dispatch.
    pub provider: Arc<dyn ProviderPort>,
    /// Tool invocation.
    pub tools: Arc<dyn ToolPort>,
    /// The session's Hands `MicroVM`.
    pub hands: Arc<dyn HandsPort>,
    /// The signed immutable catalog every capability lookup resolves against.
    pub catalog: Arc<dyn CatalogPort>,
    /// The only two clock readings the Brain takes.
    pub clock: Arc<dyn ClockPort>,
    /// Identifier generation.
    pub ids: Arc<dyn IdPort>,
}

impl core::fmt::Debug for Ports {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Ports are trait objects with no useful representation; naming the struct keeps
        // `Debug` derivable on everything that holds one without pretending to show state.
        formatter.write_str("Ports { .. }")
    }
}

/// The bounds one activation runs under.
///
/// Nothing here names a resource, so a default is safe: every value is a policy the load
/// gates size, not a binding that decides which plane the process talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationPolicy {
    /// How long a claim is taken for.
    pub lease_ttl: core::time::Duration,
    /// How often a live activation renews its claim.
    pub renew_interval: core::time::Duration,
    /// How far a progressing queue delivery is kept invisible on each renewal.
    pub visibility_timeout: core::time::Duration,
    /// The bounds one journal page read runs under.
    pub read: ReadBudget,
    /// The strict total journal restore ceiling across every page.
    pub restore: RestoreBudget,
    /// The context view policy.
    pub context: ContextPolicy,
    /// How long one external effect attempt may run, in milliseconds.
    pub effect_deadline_ms: i64,
    /// The most bytes one provider stream may buffer.
    pub stream_buffer_bytes: usize,
    /// The most bytes one provider response may carry.
    pub stream_response_bytes: usize,
    /// The longest gap between two frames before a stream is considered stalled.
    pub stream_idle_timeout_ms: u32,
    /// The most deliveries one receive asks for.
    pub receive_batch: usize,
    /// How long a receive long-polls.
    pub long_poll: core::time::Duration,
    /// How long a delivery that lost a local or durable race stays invisible.
    pub requeue_after: core::time::Duration,
    /// The most decisions one activation commits before handing the agent back.
    ///
    /// A bound rather than a budget: the planner's own limits are what stop a run, and this
    /// only stops one activation from holding a lease indefinitely while it makes progress.
    /// Hitting it commits a continuation wake, so the work is never dropped.
    pub max_steps_per_activation: u32,
    /// How many deliveries of one wake are tolerated before it is treated as poison.
    pub max_receives: u32,
    /// How many due shards the `regional-work` table is partitioned into.
    pub due_shards: u16,
    /// Minimum time between bounded due-backstop bursts.
    pub due_scan_interval: core::time::Duration,
    /// How many rotating shards one due-backstop burst covers.
    pub due_scan_shards_per_pass: u16,
    /// The most due rows one shard contributes to one burst.
    pub due_scan_page: usize,
    /// How many provider attempts one activation may make.
    ///
    /// Only a [`DispatchProof::NotSent`](aex_brain_domain::effect::DispatchProof::NotSent)
    /// failure consumes one: nothing else is retryable, so this bounds a genuine
    /// pre-dispatch loop and can never turn a possibly-served request into a second one.
    pub max_provider_attempts: u16,
}

impl Default for ActivationPolicy {
    fn default() -> Self {
        Self {
            lease_ttl: core::time::Duration::from_secs(15),
            renew_interval: core::time::Duration::from_secs(5),
            visibility_timeout: core::time::Duration::from_secs(30),
            read: ReadBudget {
                max_entries: 256,
                max_bytes: 8 * 1_024 * 1_024,
            },
            // This fail-closed ceiling is intentionally small enough that the candidate
            // mux's 1 GiB context pool can reserve it at the 100-activation target. A
            // verified snapshot plus bounded suffix is the path for histories above it.
            restore: RestoreBudget {
                max_entries: 4_096,
                max_bytes: 8 * 1_024 * 1_024,
            },
            context: ContextPolicy::default(),
            effect_deadline_ms: 600_000,
            stream_buffer_bytes: 1_024 * 1_024,
            stream_response_bytes: 8 * 1_024 * 1_024,
            stream_idle_timeout_ms: 30_000,
            receive_batch: 10,
            long_poll: core::time::Duration::from_secs(20),
            requeue_after: core::time::Duration::from_millis(250),
            max_steps_per_activation: 16,
            max_receives: 5,
            // Must match the strict-v1 regional-work descriptor. The application keeps the
            // value explicit so a migration cannot silently change the sweep topology.
            due_shards: 64,
            // SQS remains the primary path. The backstop runs a bounded 16-shard burst no
            // more than once per long-poll window, recovering at most one exceptional
            // lost-hint row from each shard. At the 20-second long poll this covers all 64
            // shards in 80 seconds, costs at most 0.8 queries and 0.8 recovered rows per
            // second per task, and cannot become hot polling while SQS stays ready.
            due_scan_interval: core::time::Duration::from_secs(20),
            due_scan_shards_per_pass: 16,
            due_scan_page: 1,
            max_provider_attempts: 3,
        }
    }
}

impl ActivationPolicy {
    /// The due shard a continuation wake for `agent` is written to.
    ///
    /// Derived from the agent identity so one agent's wakes stay in one shard and the due
    /// scan reads them in one query, rather than being spread by a counter nobody owns. The
    /// trailing bytes of a version-7 identifier are its random field, so the derivation
    /// spreads without a hash.
    #[must_use]
    pub const fn shard_for(&self, agent: aex_brain_domain::ids::AgentId) -> WorkShard {
        let shards = if self.due_shards == 0 {
            1
        } else {
            self.due_shards
        };
        let bytes = agent.0.as_bytes();
        let seed = u16::from_be_bytes([bytes[14], bytes[15]]);
        WorkShard(seed % shards)
    }
}

/// Whether the task may take more work, and what an admitted activation holds.
///
/// A trait rather than a concrete controller because the bands, the memory split and the
/// permit set are the composition root's to size. What this crate insists on is only that
/// every refusal is typed and leaves the durable wake in place.
pub trait AdmissionControl: Send + Sync + 'static {
    /// Whether the task should keep pulling from the queue.
    fn should_receive(&self) -> bool;

    /// Decides whether one activation may start.
    fn admit(&self, restore_bytes: u64) -> AdmissionDecision;
}

/// What admission decided.
#[derive(Debug)]
pub enum AdmissionDecision {
    /// Admitted, holding every permit the activation needs for its whole life.
    Admitted(Vec<crate::kernel::Reservation>),
    /// Not now. The delivery goes back; the durable wake is untouched.
    Deferred {
        /// How long before the delivery becomes visible again.
        requeue_after: core::time::Duration,
    },
    /// Refused. Also retryable — shedding is a scheduling decision, never a verdict about
    /// the work.
    Shed {
        /// How long before the delivery becomes visible again.
        retry_after: core::time::Duration,
    },
}

/// Why an activation gave the delivery back instead of acking it.
///
/// Every arm leaves the durable wake in place, so the worst any of them costs is a
/// redelivery. There is deliberately no arm meaning "dropped".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Another activation for the same agent is already running in this process.
    LocallyBusy,
    /// Another owner holds a live lease.
    HeldByOther,
    /// The process is draining and admits nothing new.
    Draining,
    /// A bounded resource was exhausted. The bytes come back; the work is admissible.
    Deferred,
    /// The agent's journal is not foldable. It does not act, and it does not ack.
    Quarantined,
}

/// How an activation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// A durable wait opened. The activation released everything and ended.
    Parked,
    /// The agent reached its absorbing terminal.
    Finished(FinishReason),
    /// The step bound was reached with work still owed, so a continuation wake was
    /// committed and the agent handed back.
    HandedBack,
}

/// What one activation concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The activation committed at least one decision, then acked.
    Progressed {
        /// How many decisions committed.
        steps: u32,
        /// How it stopped.
        stop: Stop,
    },
    /// The agent owed nothing this wake was about. The delivery was acked: the wake is
    /// satisfied, and leaving it would have the queue redeliver it forever.
    Idle,
    /// The delivery went back to the queue.
    Released(Release),
    /// The delivery has been received too many times and names work this build cannot make
    /// progress on. It is acked and recorded rather than redelivered forever.
    Poisoned {
        /// How many times it had been delivered.
        receives: u32,
    },
}

/// Why an activation could not run.
///
/// Each arm names one refusal. "Something failed" would leave the caller unable to decide
/// between releasing the delivery, quarantining the agent and failing the process.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActivationError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A conditional write was refused.
    #[error(transparent)]
    Commit(#[from] CommitError),
    /// The agent could not be claimed.
    #[error(transparent)]
    Claim(#[from] ClaimError),
    /// The journal did not fold.
    #[error(transparent)]
    Fold(#[from] FoldError),
    /// A capability lookup failed.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// A tool could not be routed.
    #[error(transparent)]
    Routing(#[from] ToolRoutingError),
    /// The Hands adapter refused.
    #[error(transparent)]
    Hands(#[from] HandsError),
    /// A record or a request could not be canonicalized, so its identity is unknown.
    #[error(transparent)]
    Canonical(#[from] aex_brain_domain::canonical::CanonicalizeError),
    /// The same effect identity arrived carrying a different request.
    ///
    /// The agent quarantines rather than dispatching either one: two requests under one
    /// identity means the derivation and the durable record disagree, and guessing which is
    /// right is how a request is sent twice.
    #[error("effect {effect} already exists under a different request hash")]
    RequestConflict {
        /// The effect.
        effect: aex_brain_domain::ids::EffectId,
    },
    /// The decision the planner owed cannot be performed by this build.
    ///
    /// Named rather than stubbed: an activation that silently did nothing would park an
    /// agent forever with no record of why.
    #[error("`{step}` is owed but unimplemented; {owed_by} supplies it")]
    Unsupported {
        /// The owed step.
        step: &'static str,
        /// Which stream owes the missing piece.
        owed_by: &'static str,
    },
}
