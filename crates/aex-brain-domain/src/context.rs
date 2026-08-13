//! Context policy: a pure view layer over the fold.
//!
//! The journal is never mutated. Everything here produces a *view* of the model-visible
//! history for one request, so a retry of the same request produces byte-identical input
//! and the provider's prompt cache is not invalidated by our own non-determinism.

use serde::{Deserialize, Serialize};

use crate::fold::FoldState;
use aex_model_catalog::primitives::BoundedString;

use crate::wire_pending::{
    CanonicalBlock, CanonicalMessage, NormalizedUsage, QualifiedModel, Role, ToolResultPart,
};

/// The text a cleared block is replaced by.
///
/// Deterministic on purpose: a retry that re-clears the same block produces the same
/// bytes, so clearing is idempotent and a re-request is a cache hit rather than a miss.
pub const CLEARED_PLACEHOLDER: &str = "[cleared by context policy]";

/// How many trailing turns are never cleared.
pub const PROTECTED_TAIL_TURNS: usize = 3;

/// The default per-tool-result byte cap.
pub const DEFAULT_TOOL_RESULT_BYTES: usize = 65_536;

/// The default hydration ceiling for one activation.
///
/// Above this, compaction is mandatory before the model call: it bounds worst-case
/// hydration memory per admitted activation, which is what makes the reservation pool a
/// real limit rather than an estimate.
pub const MAX_HYDRATED_CONTEXT_BYTES: usize = 8 * 1_024 * 1_024;

/// The tunable part of the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPolicy {
    /// Trigger compaction when prompt tokens reach this fraction of the window, in
    /// hundredths.
    pub trigger_percent_of_window: u32,
    /// Compact down to this fraction of the window, in hundredths.
    pub target_percent_of_window: u32,
    /// Per-tool-result byte cap.
    pub tool_result_bytes: usize,
    /// Hydration ceiling for one activation.
    pub max_hydrated_context_bytes: usize,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            trigger_percent_of_window: 80,
            target_percent_of_window: 50,
            tool_result_bytes: DEFAULT_TOOL_RESULT_BYTES,
            max_hydrated_context_bytes: MAX_HYDRATED_CONTEXT_BYTES,
        }
    }
}

/// What the policy decided for one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDecision {
    /// Whether a compaction pass is owed before the model call.
    pub compaction_owed: bool,
    /// Whether compaction is mandatory because hydration would exceed the ceiling.
    pub compaction_mandatory: bool,
    /// The prompt tokens the trigger was evaluated on.
    pub prompt_tokens: u64,
    /// The token budget one batched pass targets.
    pub target_tokens: u64,
    /// Whether the stable prefix is long enough for the provider to cache it.
    pub anchor_cacheable: bool,
    /// How many turns were left untouched at the tail.
    pub protected_turns: usize,
}

/// Decides what the context layer owes for one request.
///
/// The trigger basis is **total prompt tokens** — input plus cache creation plus cache
/// read — because all three occupy the window. Triggering on input tokens alone
/// under-counts a cached prompt and never compacts it.
#[must_use]
pub fn decide(
    state: &FoldState,
    model: &QualifiedModel,
    policy: &ContextPolicy,
    hydrated_bytes: usize,
) -> ContextDecision {
    let prompt_tokens = state.usage.prompt_tokens();
    let window = u64::from(model.limits().context_window_tokens).max(1);
    let trigger = window.saturating_mul(u64::from(policy.trigger_percent_of_window)) / 100;
    let target = window.saturating_mul(u64::from(policy.target_percent_of_window)) / 100;
    let mandatory = hydrated_bytes > policy.max_hydrated_context_bytes;
    let stable_prefix = stable_prefix_tokens(state, prompt_tokens);
    let min_cacheable = u64::from(model.dialect().min_cacheable_prefix_tokens());
    ContextDecision {
        compaction_owed: prompt_tokens >= trigger || mandatory,
        compaction_mandatory: mandatory,
        prompt_tokens,
        target_tokens: target,
        anchor_cacheable: min_cacheable == 0 || stable_prefix >= min_cacheable,
        protected_turns: state.model_history.len().min(PROTECTED_TAIL_TURNS),
    }
}

/// The stable prefix, in tokens, that a provider could cache.
///
/// The prefix is everything before the protected tail; the estimate is proportional
/// because the fold does not tokenize. It is used only to raise the `anchor.cacheable`
/// flag, never to bill or to size a request.
fn stable_prefix_tokens(state: &FoldState, prompt_tokens: u64) -> u64 {
    let turns = state.model_history.len();
    if turns <= PROTECTED_TAIL_TURNS {
        return 0;
    }
    let stable = u64::try_from(turns - PROTECTED_TAIL_TURNS).unwrap_or(0);
    let total = u64::try_from(turns).unwrap_or(1).max(1);
    prompt_tokens.saturating_mul(stable) / total
}

/// Produces the model-visible turns for one request.
///
/// The result is a *view*: the journal is untouched, and calling this twice on the same
/// fold produces identical bytes.
#[must_use]
pub fn view(
    state: &FoldState,
    policy: &ContextPolicy,
    clear_before: usize,
) -> Vec<CanonicalMessage> {
    let protected_from = state
        .model_history
        .len()
        .saturating_sub(PROTECTED_TAIL_TURNS);
    let clear_before = clear_before.min(protected_from);
    state
        .model_history
        .iter()
        .enumerate()
        .map(|(index, turn)| {
            if index < clear_before {
                cleared(turn)
            } else {
                capped(turn, policy.tool_result_bytes)
            }
        })
        .collect()
}

fn cleared(turn: &CanonicalMessage) -> CanonicalMessage {
    CanonicalMessage {
        role: turn.role,
        blocks: vec![CanonicalBlock::Text {
            text: BoundedString::truncating(CLEARED_PLACEHOLDER),
            annotations: Vec::new(),
        }],
    }
}

fn capped(turn: &CanonicalMessage, tool_result_bytes: usize) -> CanonicalMessage {
    CanonicalMessage {
        role: turn.role,
        blocks: match turn.role {
            Role::User => turn
                .blocks
                .iter()
                .map(|block| cap_block(block, tool_result_bytes))
                .collect(),
            Role::Assistant => turn.blocks.clone(),
        },
    }
}

fn cap_block(block: &CanonicalBlock, limit: usize) -> CanonicalBlock {
    let CanonicalBlock::ToolResult {
        call,
        content,
        is_error,
    } = block
    else {
        return block.clone();
    };
    let mut spent = 0_usize;
    let capped = content
        .iter()
        .map(|piece| match piece {
            ToolResultPart::Text { text } => {
                let remaining = limit.saturating_sub(spent);
                spent = spent.saturating_add(text.len());
                if text.len() <= remaining {
                    ToolResultPart::Text { text: text.clone() }
                } else {
                    ToolResultPart::Text {
                        text: truncate_on_char_boundary(text.as_str(), remaining),
                    }
                }
            }
            ToolResultPart::Json { value } => {
                let remaining = limit.saturating_sub(spent);
                spent = spent.saturating_add(value.as_str().len());
                if value.as_str().len() <= remaining {
                    ToolResultPart::Json {
                        value: value.clone(),
                    }
                } else {
                    ToolResultPart::Text {
                        text: BoundedString::truncating(CLEARED_PLACEHOLDER),
                    }
                }
            }
        })
        .collect();
    CanonicalBlock::ToolResult {
        call: call.clone(),
        content: capped,
        is_error: *is_error,
    }
}

/// Truncates to at most `limit` bytes without splitting a character.
///
/// The marker is charged **against** the budget rather than appended after it. If it were
/// appended, a capped result would be `limit + marker` bytes long, a second pass would find
/// it over the limit and cut it again, and the cap would not be idempotent — so a retried
/// request would send the model a different prompt than the first attempt did.
fn truncate_on_char_boundary(
    text: &str,
    limit: usize,
) -> BoundedString<{ aex_model_catalog::canonical::TEXT_MAX }> {
    if text.len() <= limit {
        return BoundedString::truncating(text);
    }
    // `CLEARED_PLACEHOLDER` is ASCII, so every byte index inside it is a character
    // boundary and the final clamp can never split a character.
    let keep = limit.saturating_sub(CLEARED_PLACEHOLDER.len());
    let mut end = keep;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = text[..end].to_owned();
    truncated.push_str(CLEARED_PLACEHOLDER);
    truncated.truncate(limit);
    BoundedString::truncating(&truncated)
}

/// The usage a compaction must carry forward.
#[must_use]
pub const fn preserved_usage(state: &FoldState) -> NormalizedUsage {
    state.usage
}

#[cfg(test)]
mod tests {
    use super::{
        CLEARED_PLACEHOLDER, ContextPolicy, MAX_HYDRATED_CONTEXT_BYTES, PROTECTED_TAIL_TURNS,
        decide, view,
    };
    use crate::fold::FoldState;
    use crate::ids::ToolCallId;
    use crate::wire_pending::{
        CanonicalBlock, CanonicalMessage, NormalizedUsage, ProviderId, Role, ToolResultPart,
    };
    use aex_model_catalog::document::CapabilitySet;
    use aex_model_catalog::{BoundedString, QualifiedModel, fixture};

    fn capability(window: u64) -> QualifiedModel {
        fixture::qualified_entry_sized(
            ProviderId::Anthropic,
            "m",
            CapabilitySet::default(),
            u32::try_from(window).expect("fixture window fits"),
            8_192,
        )
    }

    fn text(value: &str) -> CanonicalBlock {
        CanonicalBlock::Text {
            text: BoundedString::truncating(value),
            annotations: Vec::new(),
        }
    }

    fn state_with(turns: usize, usage: NormalizedUsage) -> FoldState {
        let mut state = FoldState::empty();
        state.usage = usage;
        for index in 0..turns {
            state.model_history.push(CanonicalMessage {
                role: if index % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                blocks: vec![text(&format!("turn{index}"))],
            });
        }
        state
    }

    #[test]
    fn the_trigger_counts_cache_creation_and_cache_read_not_input_alone() {
        let cached = NormalizedUsage {
            input_tokens: 10,
            cache_write_input_tokens: 400,
            cache_read_input_tokens: 400,
            ..NormalizedUsage::default()
        };
        let decision = decide(
            &state_with(4, cached),
            &capability(1_000),
            &ContextPolicy::default(),
            0,
        );
        assert_eq!(decision.prompt_tokens, 810);
        assert!(
            decision.compaction_owed,
            "a cached prompt still occupies the window"
        );
    }

    #[test]
    fn one_batched_pass_targets_half_the_window() {
        let decision = decide(
            &state_with(4, NormalizedUsage::default()),
            &capability(1_000),
            &ContextPolicy::default(),
            0,
        );
        assert_eq!(decision.target_tokens, 500);
    }

    #[test]
    fn hydration_above_the_ceiling_makes_compaction_mandatory() {
        let decision = decide(
            &state_with(2, NormalizedUsage::default()),
            &capability(1_000_000),
            &ContextPolicy::default(),
            MAX_HYDRATED_CONTEXT_BYTES + 1,
        );
        assert!(decision.compaction_mandatory);
        assert!(decision.compaction_owed);
    }

    #[test]
    fn a_short_stable_prefix_raises_the_cacheable_flag_loudly() {
        let short = decide(
            &state_with(
                4,
                NormalizedUsage {
                    input_tokens: 100,
                    ..NormalizedUsage::default()
                },
            ),
            &capability(1_000),
            &ContextPolicy::default(),
            0,
        );
        assert!(!short.anchor_cacheable);
        let cached = decide(
            &state_with(
                4,
                NormalizedUsage {
                    input_tokens: 4_200,
                    ..NormalizedUsage::default()
                },
            ),
            &capability(1_000),
            &ContextPolicy::default(),
            0,
        );
        assert!(
            cached.anchor_cacheable,
            "a stable prefix above the dialect's cacheable floor may anchor"
        );
    }

    #[test]
    fn the_last_three_turns_are_never_cleared() {
        let state = state_with(10, NormalizedUsage::default());
        let rendered = view(&state, &ContextPolicy::default(), 10);
        let tail = &rendered[rendered.len() - PROTECTED_TAIL_TURNS..];
        for turn in tail {
            assert!(
                !matches!(&turn.blocks[0], CanonicalBlock::Text { text, .. } if text.as_str() == CLEARED_PLACEHOLDER),
                "{turn:?}"
            );
        }
    }

    #[test]
    fn clearing_is_idempotent_so_a_retry_is_byte_identical() {
        let state = state_with(8, NormalizedUsage::default());
        let once = view(&state, &ContextPolicy::default(), 4);
        let twice = view(&state, &ContextPolicy::default(), 4);
        assert_eq!(once, twice);
    }

    fn only_text(turns: &[CanonicalMessage]) -> &str {
        assert_eq!(turns[0].role, Role::User);
        let CanonicalBlock::ToolResult { content, .. } = &turns[0].blocks[0] else {
            panic!("a tool result");
        };
        let ToolResultPart::Text { text } = &content[0] else {
            panic!("text content");
        };
        text.as_str()
    }

    #[test]
    fn a_tool_result_is_capped_at_the_effective_limit_and_capping_is_idempotent() {
        let limit = CLEARED_PLACEHOLDER.len() + 16;
        let mut state = FoldState::empty();
        state.model_history.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: ToolCallId::truncating("c1"),
                content: vec![ToolResultPart::Text {
                    text: BoundedString::truncating(&"x".repeat(200)),
                }],
                is_error: false,
            }],
        });
        let policy = ContextPolicy {
            tool_result_bytes: limit,
            ..ContextPolicy::default()
        };

        let once = view(&state, &policy, 0);
        let text = only_text(&once);
        // The marker is charged against the budget, so the capped result is at or below the
        // declared limit. A marker appended past the limit would make the limit a lie and
        // would make a second pass cut the result again.
        assert!(
            text.len() <= limit,
            "{} bytes for a {limit} limit",
            text.len()
        );
        assert!(text.starts_with(&"x".repeat(16)));
        assert!(text.ends_with(CLEARED_PLACEHOLDER));

        // Real idempotence: feed the capped result back in, not the original.
        let mut recapped = FoldState::empty();
        recapped.model_history.clone_from(&once);
        assert_eq!(once, view(&recapped, &policy, 0));
    }

    #[test]
    fn a_result_already_inside_the_limit_is_returned_untouched() {
        let mut state = FoldState::empty();
        state.model_history.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: ToolCallId::truncating("c1"),
                content: vec![ToolResultPart::Text {
                    text: BoundedString::truncating("small"),
                }],
                is_error: false,
            }],
        });
        let viewed = view(&state, &ContextPolicy::default(), 0);
        assert_eq!(only_text(&viewed), "small");
    }

    #[test]
    fn the_journal_is_never_mutated_by_producing_a_view() {
        let state = state_with(6, NormalizedUsage::default());
        let before = state.clone();
        let _ = view(&state, &ContextPolicy::default(), 3);
        assert_eq!(state, before);
    }
}
