//! The subagent scheduler.
//!
//! Four rules shape everything here, and each exists because of a specific way the obvious
//! design goes wrong.
//!
//! - **`create_subagent` never blocks and never fails for lack of capacity.** A caller that
//!   cannot tell "refused" from "not yet scheduled" retries, and a retry creates a second
//!   child. So a spawn always returns a durable id and a state, and a child that cannot run
//!   yet is `Queued` with a visible reason.
//! - **Child ids are derived, never minted.** `blake3(parent || ordinal)` means a retried
//!   fanout page produces exactly the same children, so the retry is idempotent without any
//!   extra durable record to remember what the first attempt did.
//! - **Dequeue before claim; fenced stop after it.** These are different operations, not one
//!   operation with a race. Before the scheduler claims a child, the parent may remove it
//!   and the history is preserved. After the claim, only a fence-carrying stop is admissible,
//!   because a stop aimed at a previous incarnation would cancel work the caller never saw.
//! - **The claim checks the limit it is about to consume, in the transaction that consumes
//!   it.** Reading the active count and then incrementing it leaves a window in which two
//!   claimants both conclude there is room.

pub mod claim;
pub mod fanout;
pub mod join;
pub mod mailbox;

pub use claim::{
    ClaimDecision, ClaimPlan, ClaimRefusal, DequeuePlan, StopPlan, StopRefusal, plan_claim,
    plan_dequeue, plan_stop,
};
pub use fanout::{
    PlannedChild, SessionCapacity, SpawnError, SpawnPage, SpawnPlan, SpawnRequest,
    grant_only_reduces, launch_structural, plan_spawn, queued_reason,
};
pub use join::{WaitOutcome, plan_wait, resolve_wait, should_wake, wake_key};
pub use mailbox::{MailboxEntry, drain, entry_digest};
