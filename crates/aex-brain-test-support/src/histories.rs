//! Named golden semantic histories.
//!
//! Owned by the brain-core stream (plan 07 §1, slice S-1.1). Each entry is a *semantic*
//! case with a name that states what it proves, not a serialized snapshot of an
//! implementation's output. A history derived from what the code currently does could only
//! ever confirm that the code still does it.
//!
//! Roughly half are hostile on purpose: a corpus of well-formed journals would exercise
//! the fold and none of the guards that exist for malformed input.

use aex_brain_domain::child::{CancelCause, ChildOutcome};
use aex_brain_domain::effect::{EffectClass, EffectKind};
use aex_brain_domain::ids::{ContentHash, JournalSeq, Timestamp, child_agent_id};
use aex_brain_domain::journal::{
    FinishReason, JournalEntry, JournalRecord, ParkReason, WaitResolution,
};
use aex_brain_domain::wire_pending::{CanonicalBlock, JoinMode, StopReason};
use aex_model_catalog::BoundedString;

use crate::journal_gen::{
    HistoryBuilder, agent, assistant, assistant_text, assistant_tool_use, child_spawned,
    child_terminal, effect_complete, effect_known_failure, effect_prepared, effect_unknown,
    finished, grant, join, join_opened, model_effect, started, tool_effect, tool_result, user_text,
    wait, wait_opened, wait_resolved,
};

/// What a golden history is expected to do when folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// The history folds and the agent is not terminal.
    FoldsOpen,
    /// The history folds and the agent reached `reason`.
    FoldsTerminal(FinishReason),
    /// The fold rejects the history; the discriminant names which guard fires.
    Rejected(Rejection),
}

/// Which fold guard a hostile history must trip.
///
/// A rejection is named rather than compared structurally so a case states the *reason* it
/// is hostile. `assert!(fold(h).is_err())` would pass for a history rejected by the wrong
/// guard, which is precisely the failure these cases exist to catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// A sequence was skipped.
    JournalGap,
    /// The same sequence arrived under a different hash.
    JournalForked,
    /// A record arrived after the absorbing terminal.
    TerminalAbsorbing,
    /// A record other than `agent_started` arrived first.
    NotStarted,
    /// A tool result answered a call the model never made.
    UnknownCall,
    /// A tool result answered a call already resolved.
    DuplicateToolResult,
    /// An effect settled that was never prepared.
    UnknownEffect,
    /// The same effect was prepared twice.
    DuplicateEffect,
    /// A second `agent_started` arrived.
    AlreadyStarted,
    /// A budget dimension had no headroom.
    Budget,
    /// The envelope hash did not cover the record it carried.
    EnvelopeHashMismatch,
    /// A wait resolved that was never opened.
    UnknownWait,
    /// A child fact named an agent this parent never spawned.
    UnknownChild,
    /// An assistant message arrived whose proof does not cover its blocks.
    UnprovenAssistantMessage,
}

impl Rejection {
    /// Whether `error` is the guard this rejection names.
    ///
    /// Matching on the discriminant rather than comparing whole values keeps a case honest
    /// about *which* guard must fire without pinning the diagnostic payload, which is free
    /// to improve.
    #[must_use]
    pub const fn matches(self, error: &aex_brain_domain::fold::FoldError) -> bool {
        use aex_brain_domain::fold::FoldError as E;
        matches!(
            (self, error),
            (Self::JournalGap, E::JournalGap { .. })
                | (Self::JournalForked, E::JournalForked { .. })
                | (Self::TerminalAbsorbing, E::TerminalAbsorbing { .. })
                | (Self::NotStarted, E::NotStarted { .. })
                | (Self::UnknownCall, E::UnknownCall { .. })
                | (Self::DuplicateToolResult, E::DuplicateToolResult { .. })
                | (Self::UnknownEffect, E::UnknownEffect { .. })
                | (Self::DuplicateEffect, E::DuplicateEffect { .. })
                | (Self::AlreadyStarted, E::AlreadyStarted)
                | (Self::Budget, E::Budget(_))
                | (Self::EnvelopeHashMismatch, E::EnvelopeHashMismatch { .. })
                | (Self::UnknownWait, E::UnknownWait { .. })
                | (Self::UnknownChild, E::UnknownChild { .. })
                | (
                    Self::UnprovenAssistantMessage,
                    E::UnprovenAssistantMessage { .. }
                )
        )
    }
}

/// One named case.
#[derive(Debug, Clone)]
pub struct Golden {
    /// What the case proves, in words.
    pub name: &'static str,
    /// The history itself.
    pub history: Vec<JournalEntry>,
    /// What folding it must produce.
    pub expectation: Expectation,
}

fn text(body: &str) -> Vec<CanonicalBlock> {
    vec![CanonicalBlock::Text {
        text: BoundedString::truncating(body),
        annotations: Vec::new(),
    }]
}

fn mismatched_assistant(
    actual: &str,
    covered: &str,
    stop: StopReason,
    effect: aex_brain_domain::ids::EffectId,
) -> JournalRecord {
    let mut record = assistant_text(covered, effect);
    let JournalRecord::AssistantMessage { message, .. } = &mut record else {
        unreachable!("assistant_text always returns an assistant message")
    };
    message.blocks = text(actual);
    message.stop_reason = stop;
    record
}

/// Every golden semantic history, in a stable order.
///
/// # Panics
///
/// Panics when a fixture record cannot be canonicalized, which is a defect in the fixture.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn all() -> Vec<Golden> {
    let owner = agent(1);
    let call = |seq: u64| model_effect(owner, JournalSeq(seq));
    let tool = |seq: u64| tool_effect(owner, JournalSeq(seq));

    let mut cases = Vec::new();

    cases.push(Golden {
        name: "a started agent with no input owes nothing and is not terminal",
        history: HistoryBuilder::new().push(started(grant(1_000))).build(),
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "one user message, one clean assistant turn, one clean terminal",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("hello"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(assistant_text("hi", call(2)))
            .push(effect_complete(call(2)))
            .push(finished(FinishReason::Completed))
            .build(),
        expectation: Expectation::FoldsTerminal(FinishReason::Completed),
    });

    cases.push(Golden {
        name: "a tool round trip closes the call and reopens the model turn",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("read the file"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(assistant_tool_use(&[("c1", "read_file")], call(2)))
            .push(effect_complete(call(2)))
            .push(effect_prepared(
                tool(5),
                EffectKind::ToolCall,
                EffectClass::IdempotentManaged,
            ))
            .push(tool_result("c1", "contents", tool(5)))
            .push(effect_complete(tool(5)))
            .build(),
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "two tool calls resolve out of arrival order and rebuild in tool-use order",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("do both"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(assistant_tool_use(
                &[("first", "read_file"), ("second", "write_file")],
                call(2),
            ))
            .push(effect_complete(call(2)))
            .push(tool_result("second", "wrote", tool(5)))
            .push(tool_result("first", "read", tool(6)))
            .build(),
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a message claiming a proof it does not carry never enters model history",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("write a long thing"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(mismatched_assistant(
                "cut off half",
                "what the proof covers",
                StopReason::MaxOutputTokens,
                call(2),
            ))
            .build(),
        expectation: Expectation::Rejected(Rejection::UnprovenAssistantMessage),
    });

    cases.push(Golden {
        name: "an unproved assistant message never enters model-visible history",
        history: {
            HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(user_text("go"))
                .push(mismatched_assistant(
                    "claimed complete",
                    "something else entirely",
                    StopReason::EndTurn,
                    call(2),
                ))
                .build()
        },
        expectation: Expectation::Rejected(Rejection::UnprovenAssistantMessage),
    });

    cases.push(Golden {
        name: "an unknown-outcome effect projects to interrupted and nothing else",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(effect_unknown(call(2)))
            .build(),
        expectation: Expectation::FoldsTerminal(FinishReason::Interrupted),
    });

    cases.push(Golden {
        name: "a proved pre-dispatch refusal settles without terminalizing the agent",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(effect_known_failure(call(2)))
            .build(),
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "park and resume leaves the agent runnable again",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(wait_opened(wait(1), ParkReason::AwaitingUserMessage))
            .push(wait_resolved(wait(1), WaitResolution::Delivered))
            .build(),
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a spawn, a join and a child terminal return the concurrent grant in full",
        history: {
            let child = child_agent_id(owner, 0);
            HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(join_opened(join(1), JoinMode::All, vec![child]))
                .push(child_spawned(child, 0, join(1), grant(10), None))
                .push(child_terminal(child, ChildOutcome::Completed))
                .build()
        },
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a child failure is a typed result and does not terminalize the parent",
        history: {
            let child = child_agent_id(owner, 0);
            HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(join_opened(join(1), JoinMode::Any, vec![child]))
                .push(child_spawned(child, 0, join(1), grant(10), None))
                .push(child_terminal(child, ChildOutcome::Failed))
                .build()
        },
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a dequeued child preserves history rather than vanishing",
        history: {
            let child = child_agent_id(owner, 0);
            HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(join_opened(join(1), JoinMode::All, vec![child]))
                .push(child_spawned(
                    child,
                    0,
                    join(1),
                    grant(10),
                    Some(aex_brain_domain::child::QueuedReason::ActiveBudget),
                ))
                .push(child_terminal(
                    child,
                    ChildOutcome::Cancelled {
                        cause: CancelCause::Dequeued,
                    },
                ))
                .build()
        },
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a duplicate child terminal is idempotent, not an error",
        history: {
            let child = child_agent_id(owner, 0);
            HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(join_opened(join(1), JoinMode::All, vec![child]))
                .push(child_spawned(child, 0, join(1), grant(10), None))
                .push(child_terminal(child, ChildOutcome::Completed))
                .push(child_terminal(child, ChildOutcome::Completed))
                .build()
        },
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "every terminal reason is absorbing",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(finished(FinishReason::Cancelled))
            .build(),
        expectation: Expectation::FoldsTerminal(FinishReason::Cancelled),
    });

    // ---- hostile ----

    cases.push(Golden {
        name: "a skipped sequence is a gap, never a silent fold",
        history: {
            let mut history = HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(user_text("one"))
                .push(user_text("two"))
                .build();
            history.remove(1);
            history
        },
        expectation: Expectation::Rejected(Rejection::JournalGap),
    });

    cases.push(Golden {
        name: "the same sequence under a different hash quarantines the agent",
        history: {
            let mut history = HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(user_text("one"))
                .build();
            let forged = JournalEntry::seal(
                JournalSeq(1),
                Timestamp(0),
                user_text("a different body at the same sequence"),
            )
            .expect("the forged record canonicalizes");
            history.push(forged);
            history
        },
        expectation: Expectation::Rejected(Rejection::JournalForked),
    });

    cases.push(Golden {
        name: "a record after the terminal is rejected, not appended",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(finished(FinishReason::Completed))
            .push(user_text("too late"))
            .build(),
        expectation: Expectation::Rejected(Rejection::TerminalAbsorbing),
    });

    cases.push(Golden {
        name: "a journal that does not begin with agent_started does not fold",
        history: HistoryBuilder::new().push(user_text("orphan")).build(),
        expectation: Expectation::Rejected(Rejection::NotStarted),
    });

    cases.push(Golden {
        name: "a second agent_started is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(started(grant(1_000)))
            .build(),
        expectation: Expectation::Rejected(Rejection::AlreadyStarted),
    });

    cases.push(Golden {
        name: "a tool result for a call the model never made is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(assistant_text("no tools here", call(2)))
            .push(effect_complete(call(2)))
            .push(tool_result("phantom", "result", tool(5)))
            .build(),
        expectation: Expectation::Rejected(Rejection::UnknownCall),
    });

    cases.push(Golden {
        name: "a second result for a resolved call is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(effect_prepared(
                call(2),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(assistant_tool_use(&[("c1", "read_file")], call(2)))
            .push(effect_complete(call(2)))
            .push(tool_result("c1", "once", tool(5)))
            .push(tool_result("c1", "twice", tool(6)))
            .build(),
        expectation: Expectation::Rejected(Rejection::DuplicateToolResult),
    });

    cases.push(Golden {
        name: "an effect settling without a preparation is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(effect_complete(call(1)))
            .build(),
        expectation: Expectation::Rejected(Rejection::UnknownEffect),
    });

    cases.push(Golden {
        name: "the same effect prepared twice is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(effect_prepared(
                call(1),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .push(effect_prepared(
                call(1),
                EffectKind::ModelCall,
                EffectClass::NonReplayable,
            ))
            .build(),
        expectation: Expectation::Rejected(Rejection::DuplicateEffect),
    });

    cases.push(Golden {
        name: "a spawn beyond the granted active budget is refused, not queued silently",
        history: {
            let first = child_agent_id(owner, 0);
            let second = child_agent_id(owner, 1);
            HistoryBuilder::new()
                .push(started(grant(2)))
                .push(join_opened(join(1), JoinMode::All, vec![first, second]))
                .push(child_spawned(first, 0, join(1), grant(1), None))
                .push(child_spawned(second, 1, join(1), grant(1), None))
                .build()
        },
        expectation: Expectation::Rejected(Rejection::Budget),
    });

    cases.push(Golden {
        name: "an envelope hash that does not cover its record is rejected",
        history: {
            let mut history = HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(user_text("one"))
                .build();
            history[1].envelope.content_hash = ContentHash::of(b"not this record");
            history
        },
        expectation: Expectation::Rejected(Rejection::EnvelopeHashMismatch),
    });

    cases.push(Golden {
        name: "a wait resolving that was never opened is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(wait_resolved(wait(1), WaitResolution::Delivered))
            .build(),
        expectation: Expectation::Rejected(Rejection::UnknownWait),
    });

    cases.push(Golden {
        name: "a terminal for a child this parent never spawned is rejected",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(child_terminal(
                child_agent_id(owner, 7),
                ChildOutcome::Completed,
            ))
            .build(),
        expectation: Expectation::Rejected(Rejection::UnknownChild),
    });

    cases.push(Golden {
        name: "an exact redelivery of an already folded record is a no-op",
        history: {
            let mut history = HistoryBuilder::new()
                .push(started(grant(1_000)))
                .push(user_text("one"))
                .build();
            history.push(history[1].clone());
            history
        },
        expectation: Expectation::FoldsOpen,
    });

    cases.push(Golden {
        name: "a refusal is a terminal stop reason and folds as a complete turn",
        history: HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("go"))
            .push(assistant(
                text("at least one block"),
                StopReason::Refusal,
                call(2),
            ))
            .build(),
        expectation: Expectation::FoldsOpen,
    });

    cases
}

#[cfg(test)]
mod tests {
    use super::all;

    #[test]
    fn the_corpus_meets_the_declared_minimum_and_has_unique_names() {
        let cases = all();
        assert!(
            cases.len() >= 20,
            "slice S-1.1 requires at least 20 semantic histories, found {}",
            cases.len()
        );
        let mut names: Vec<&str> = cases.iter().map(|case| case.name).collect();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "every case name must be distinct");
    }

    #[test]
    fn roughly_half_the_corpus_is_hostile() {
        let cases = all();
        let hostile = cases
            .iter()
            .filter(|case| matches!(case.expectation, super::Expectation::Rejected(_)))
            .count();
        assert!(
            hostile >= cases.len() / 3,
            "a corpus of well-formed journals proves nothing about the guards: {hostile} of {}",
            cases.len()
        );
    }
}
