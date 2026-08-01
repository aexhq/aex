//! The planner: a total function from fold state to the one step the agent owes.
//!
//! Recovery is not a mode. A new owner claims, folds and calls [`plan`]; there is no
//! separate replay path that could disagree with the normal one.
//!
//! Enforcement runs **before** any I/O. Turn deadline, max turns, max steps and budget
//! breach each map to an honest [`FinishReason`], so a run that cannot legally continue
//! never reserves a permit or opens a socket first.

use serde::{Deserialize, Serialize};

use crate::budget::{DIMENSIONS, Dimension};
use crate::child::QueuedReason;
use crate::effect::{EffectKind, EffectState};
use crate::fold::{FoldState, PendingCall, Phase};
use crate::ids::{EffectId, JoinId, Timestamp};
use crate::journal::{FinishReason, ParkReason};
use crate::wire_pending::StopReason;

/// The one step the agent owes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum OwedStep {
    /// Call the model.
    ModelCall {
        /// The deterministic identity the effect will carry.
        effect: EffectId,
    },
    /// Run the outstanding tool calls.
    ToolCalls {
        /// The calls, in tool-use order.
        calls: Vec<PendingCall>,
    },
    /// Commit a page of children.
    SpawnChildren {
        /// How many children this page admits.
        page: u32,
        /// The join group they belong to.
        join: JoinId,
    },
    /// Park on a durable wait.
    Park {
        /// Why.
        reason: Box<ParkReason>,
    },
    /// Commit the terminal.
    Finish {
        /// Why.
        reason: FinishReason,
    },
    /// Already terminal; nothing is owed.
    Finished {
        /// Why it finished.
        reason: FinishReason,
    },
}

/// What the planner is allowed to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPolicy {
    /// Wall clock at the moment of planning.
    pub now: Timestamp,
    /// The session cancellation epoch the agent was claimed under.
    pub cancel_requested: bool,
    /// Maximum assistant turns.
    pub max_turns: u32,
    /// Maximum planner steps inside one turn.
    pub max_steps_per_turn: u32,
    /// Wall-clock ceiling for one turn, in milliseconds.
    pub turn_deadline_ms: u32,
}

impl PlanPolicy {
    /// A policy from the agent's pinned limits at instant `now`.
    #[must_use]
    pub const fn from_limits(now: Timestamp, limits: crate::wire_pending::AgentLimits) -> Self {
        Self {
            now,
            cancel_requested: false,
            max_turns: limits.max_turns,
            max_steps_per_turn: limits.max_steps_per_turn,
            turn_deadline_ms: limits.turn_deadline_ms,
        }
    }
}

/// The step `state` owes under `policy`.
///
/// Total: every fold state maps to exactly one step, which is what makes recovery derived
/// rather than replayed.
#[must_use]
pub fn plan(state: &FoldState, policy: &PlanPolicy) -> OwedStep {
    if let Some(reason) = state.finish {
        return OwedStep::Finished { reason };
    }

    // A truncating provider stop finishes `Failed`, never `Completed`. Reporting success on
    // a cut-off deliverable is a correctness bug, so this outranks every other check that
    // could otherwise close the turn cleanly.
    if truncated(state) {
        return OwedStep::Finish {
            reason: FinishReason::Failed,
        };
    }

    if policy.cancel_requested {
        return OwedStep::Finish {
            reason: FinishReason::Cancelled,
        };
    }

    if let Some(reason) = breach(state, policy) {
        return OwedStep::Finish { reason };
    }

    match &state.phase {
        Phase::Finished => OwedStep::Finished {
            reason: state.finish.unwrap_or(FinishReason::Completed),
        },
        Phase::Parked { reason } => OwedStep::Park {
            reason: reason.clone(),
        },
        Phase::Effecting { effect } => match state.open_effects.get(effect) {
            // An effect the previous owner only prepared is retryable under a new fence.
            Some(EffectState::Prepared { .. }) | None => OwedStep::ModelCall { effect: *effect },
            // Anything dispatched is decided by `effect::recover`, not by the planner: the
            // planner has no dispatch evidence and must not guess.
            Some(_) => OwedStep::Park {
                reason: Box::new(ParkReason::AwaitingCapacity {
                    reason: QueuedReason::RegionalCapacity,
                }),
            },
        },
        Phase::AwaitingTools => OwedStep::ToolCalls {
            calls: state.pending_in_order(),
        },
        Phase::AwaitingInput => OwedStep::Park {
            reason: Box::new(ParkReason::AwaitingUserMessage),
        },
        Phase::AwaitingModel => OwedStep::ModelCall {
            effect: EffectId::derive(
                state.parent.unwrap_or_else(|| {
                    // The effect identity is derived from the agent's own id; a root agent
                    // has no parent, and the store adapter supplies the real agent id. The
                    // planner derives from the sequence and kind so the same owed step at
                    // the same tail always yields the same identity.
                    crate::ids::AgentId(uuid::Uuid::nil())
                }),
                state.expected_seq(),
                EffectKind::ModelCall.tag(),
            ),
        },
        Phase::AwaitingFinish => OwedStep::Finish {
            reason: FinishReason::Completed,
        },
    }
}

/// Derives the model-call effect identity for `agent` at this fold's next sequence.
///
/// The planner cannot know the agent's own id from the fold alone, so the orchestrator
/// resolves it here. Keeping the derivation in one function means the retried step and the
/// original derive the same identity.
#[must_use]
pub fn model_effect_id(agent: crate::ids::AgentId, state: &FoldState) -> EffectId {
    EffectId::derive(agent, state.expected_seq(), EffectKind::ModelCall.tag())
}

fn truncated(state: &FoldState) -> bool {
    state.model_history.last().is_some_and(|turn| match turn {
        crate::wire_pending::Turn::Assistant { stop_reason, .. } => {
            matches!(stop_reason, StopReason::MaxTokens)
        }
        crate::wire_pending::Turn::User { .. } => false,
    }) && state.pending_calls.is_empty()
}

fn breach(state: &FoldState, policy: &PlanPolicy) -> Option<FinishReason> {
    if state.assistant_turns >= policy.max_turns {
        return Some(FinishReason::MaxTurns);
    }
    if state.steps_this_turn >= policy.max_steps_per_turn {
        return Some(FinishReason::MaxSteps);
    }
    if let Some(started) = state.turn_started_at {
        let elapsed = policy.now.millis().saturating_sub(started.millis());
        if elapsed >= i64::from(policy.turn_deadline_ms) {
            return Some(FinishReason::Timeout);
        }
    }
    if DIMENSIONS
        .iter()
        .any(|&dimension| exhausted(state, dimension))
    {
        return Some(FinishReason::Budget);
    }
    None
}

fn exhausted(state: &FoldState, dimension: Dimension) -> bool {
    // `ProviderCalls` is derived from cost and the run deadline rather than being an
    // independent product cap, so a zero limit there means "unset", not "exhausted".
    if dimension == Dimension::ProviderCalls && state.budget.limit.get(dimension) == 0 {
        return false;
    }
    state.budget.free(dimension) == 0 && state.budget.limit.get(dimension) > 0
}

#[cfg(test)]
mod tests {
    use super::{OwedStep, PlanPolicy, plan};
    use crate::budget::{BudgetNode, DimensionVector};
    use crate::fold::{FoldState, Phase};
    use crate::ids::{ModelSlug, Timestamp};
    use crate::journal::{FinishReason, ParkReason};
    use crate::wire_pending::{CanonicalBlock, ProviderId, StopReason, Turn};

    fn policy() -> PlanPolicy {
        PlanPolicy {
            now: Timestamp::from_millis(0),
            cancel_requested: false,
            max_turns: 100,
            max_steps_per_turn: 100,
            turn_deadline_ms: 60_000,
        }
    }

    fn state() -> FoldState {
        FoldState {
            budget: BudgetNode::root(DimensionVector::uniform(1_000)),
            ..FoldState::empty()
        }
    }

    fn assistant(stop_reason: StopReason) -> Turn {
        Turn::Assistant {
            blocks: vec![CanonicalBlock::Text {
                text: "partial".to_owned(),
            }],
            provider: ProviderId::Anthropic,
            model: ModelSlug("m".to_owned()),
            stop_reason,
        }
    }

    #[test]
    fn a_truncated_response_finishes_failed_never_completed() {
        let mut folded = state();
        folded.model_history.push(assistant(StopReason::MaxTokens));
        folded.phase = Phase::AwaitingFinish;
        assert_eq!(
            plan(&folded, &policy()),
            OwedStep::Finish {
                reason: FinishReason::Failed
            }
        );
    }

    #[test]
    fn a_clean_stop_finishes_completed() {
        let mut folded = state();
        folded.model_history.push(assistant(StopReason::EndTurn));
        folded.phase = Phase::AwaitingFinish;
        assert_eq!(
            plan(&folded, &policy()),
            OwedStep::Finish {
                reason: FinishReason::Completed
            }
        );
    }

    #[test]
    fn each_breach_maps_to_its_own_honest_reason() {
        let mut turns = state();
        turns.assistant_turns = 5;
        turns.phase = Phase::AwaitingModel;
        assert_eq!(
            plan(
                &turns,
                &PlanPolicy {
                    max_turns: 5,
                    ..policy()
                }
            ),
            OwedStep::Finish {
                reason: FinishReason::MaxTurns
            }
        );

        let mut steps = state();
        steps.steps_this_turn = 7;
        steps.phase = Phase::AwaitingModel;
        assert_eq!(
            plan(
                &steps,
                &PlanPolicy {
                    max_steps_per_turn: 7,
                    ..policy()
                }
            ),
            OwedStep::Finish {
                reason: FinishReason::MaxSteps
            }
        );

        let mut timed = state();
        timed.turn_started_at = Some(Timestamp::from_millis(0));
        timed.phase = Phase::AwaitingModel;
        assert_eq!(
            plan(
                &timed,
                &PlanPolicy {
                    now: Timestamp::from_millis(60_000),
                    ..policy()
                }
            ),
            OwedStep::Finish {
                reason: FinishReason::Timeout
            }
        );

        let mut broke = state();
        broke.budget = BudgetNode::root(DimensionVector::uniform(1));
        broke
            .budget
            .consume(crate::budget::Dimension::CostMicroUsd, 1)
            .expect("exactly the limit");
        broke.phase = Phase::AwaitingModel;
        assert_eq!(
            plan(&broke, &policy()),
            OwedStep::Finish {
                reason: FinishReason::Budget
            }
        );
    }

    #[test]
    fn cancellation_outranks_every_continuation() {
        let mut folded = state();
        folded.phase = Phase::AwaitingModel;
        assert_eq!(
            plan(
                &folded,
                &PlanPolicy {
                    cancel_requested: true,
                    ..policy()
                }
            ),
            OwedStep::Finish {
                reason: FinishReason::Cancelled
            }
        );
    }

    #[test]
    fn a_terminal_agent_owes_nothing() {
        let mut folded = state();
        folded.finish = Some(FinishReason::Completed);
        folded.phase = Phase::Finished;
        assert_eq!(
            plan(
                &folded,
                &PlanPolicy {
                    cancel_requested: true,
                    ..policy()
                }
            ),
            OwedStep::Finished {
                reason: FinishReason::Completed
            }
        );
    }

    #[test]
    fn an_agent_with_no_input_parks_rather_than_calling_a_model() {
        assert_eq!(
            plan(&state(), &policy()),
            OwedStep::Park {
                reason: Box::new(ParkReason::AwaitingUserMessage)
            }
        );
    }
}
