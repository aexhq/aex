//! Stream bounds and their typed overruns (plan 08 §3.3).
//!
//! Every bound here is a **shared-safety default** in the sense of
//! `references/limits-and-ceilings-decision-2026-07-30.md`, not a protocol
//! fact: it is configuration, it is reported on the receipt, and an overrun is
//! a typed failure — never a truncation that reports success (D-28).

use core::time::Duration;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::primitives::ToolCallId;

use crate::wire_pending::ReservationSet;

/// Time to establish a connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Time to the response head.
pub const DEFAULT_HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Time from the head to the first decoded dialect frame.
pub const DEFAULT_FIRST_FRAME_TIMEOUT: Duration = Duration::from_mins(2);
/// Time permitted between frames.
pub const DEFAULT_IDLE_FRAME_TIMEOUT: Duration = Duration::from_mins(1);

/// One frame.
pub const DEFAULT_MAX_FRAME_BYTES: u32 = 1024 * 1024;
/// One whole response.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
/// One error body. Read to here and no further.
pub const DEFAULT_MAX_ERROR_BODY_BYTES: u32 = 16 * 1024;
/// Blocks in one turn.
pub const DEFAULT_MAX_BLOCKS: u16 = 512;
/// Tool calls in one turn.
pub const DEFAULT_MAX_TOOL_CALLS: u16 = 128;
/// Assistant text in one turn.
pub const DEFAULT_MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
/// Reasoning text in one turn.
pub const DEFAULT_MAX_REASONING_BYTES: u64 = 8 * 1024 * 1024;
/// Arguments for one tool call.
pub const DEFAULT_MAX_TOOL_ARGUMENT_BYTES: u32 = 1024 * 1024;

/// The bounds one dispatch runs under.
#[derive(Debug)]
pub struct StreamBudget {
    /// Time to establish a connection.
    pub connect_timeout: Duration,
    /// Time to the response head.
    pub head_timeout: Duration,
    /// Time from the head to the first decoded dialect frame.
    pub first_frame_timeout: Duration,
    /// Time permitted between frames.
    pub idle_frame_timeout: Duration,
    /// The whole-dispatch deadline, derived from the effect deadline.
    pub total_deadline: Duration,
    /// One frame.
    pub max_frame_bytes: u32,
    /// One whole response.
    pub max_response_bytes: u64,
    /// One error body.
    pub max_error_body_bytes: u32,
    /// Blocks in one turn.
    pub max_blocks: u16,
    /// Tool calls in one turn.
    pub max_tool_calls: u16,
    /// Assistant text in one turn.
    pub max_text_bytes: u64,
    /// Reasoning text in one turn.
    pub max_reasoning_bytes: u64,
    /// Arguments for one tool call.
    pub max_tool_argument_bytes: u32,
    /// Memory permits the mux minted for this dispatch.
    pub reservations: ReservationSet,
}

impl Default for StreamBudget {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            head_timeout: DEFAULT_HEAD_TIMEOUT,
            first_frame_timeout: DEFAULT_FIRST_FRAME_TIMEOUT,
            idle_frame_timeout: DEFAULT_IDLE_FRAME_TIMEOUT,
            total_deadline: Duration::from_mins(15),
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_error_body_bytes: DEFAULT_MAX_ERROR_BODY_BYTES,
            max_blocks: DEFAULT_MAX_BLOCKS,
            max_tool_calls: DEFAULT_MAX_TOOL_CALLS,
            max_text_bytes: DEFAULT_MAX_TEXT_BYTES,
            max_reasoning_bytes: DEFAULT_MAX_REASONING_BYTES,
            max_tool_argument_bytes: DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
            reservations: ReservationSet::default(),
        }
    }
}

impl StreamBudget {
    /// Narrows the byte and time bounds to the tighter of the shared default
    /// and whatever the catalog entry declares.
    ///
    /// The catalog can only make a bound *smaller*: a document must not be able
    /// to widen a shared-safety default, or a mistaken publish would raise the
    /// process's own memory ceiling.
    #[must_use]
    pub fn narrowed_to(mut self, model: &QualifiedModel) -> Self {
        let limits = model.limits();
        self.max_frame_bytes = self.max_frame_bytes.min(limits.response_frame_max_bytes);
        self.idle_frame_timeout = self.idle_frame_timeout.min(Duration::from_millis(u64::from(
            limits.stream_idle_timeout_ms,
        )));
        self.total_deadline = self.total_deadline.min(Duration::from_millis(u64::from(
            limits.total_stream_deadline_ms,
        )));
        self
    }
}

/// Which bound a dispatch crossed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetOverrun {
    /// One frame exceeded its bound.
    #[error("a frame of {seen} bytes exceeds the {limit}-byte bound")]
    Frame {
        /// The bound.
        limit: u32,
        /// What arrived.
        seen: u32,
    },
    /// The whole response exceeded its bound.
    #[error("the response exceeds the {limit}-byte bound")]
    Response {
        /// The bound.
        limit: u64,
    },
    /// Too many blocks in one turn.
    #[error("the turn exceeds the {limit}-block bound")]
    Blocks {
        /// The bound.
        limit: u16,
    },
    /// Too many tool calls in one turn.
    #[error("the turn exceeds the {limit}-tool-call bound")]
    ToolCalls {
        /// The bound.
        limit: u16,
    },
    /// Too much assistant text.
    #[error("the turn exceeds the {limit}-byte text bound")]
    Text {
        /// The bound.
        limit: u64,
    },
    /// Too much reasoning text.
    #[error("the turn exceeds the {limit}-byte reasoning bound")]
    Reasoning {
        /// The bound.
        limit: u64,
    },
    /// One tool call's arguments exceeded their bound.
    #[error("arguments for call `{call}` exceed the {limit}-byte bound")]
    ToolArguments {
        /// Which call.
        call: ToolCallId,
        /// The bound.
        limit: u32,
    },
    /// No frame arrived within the idle window.
    #[error("no frame arrived within {}ms", .after.as_millis())]
    IdleFrame {
        /// The window.
        after: Duration,
    },
    /// No first frame arrived within its window.
    #[error("no first frame arrived within {}ms", .after.as_millis())]
    FirstFrame {
        /// The window.
        after: Duration,
    },
    /// The response head did not arrive within its window.
    #[error("no response head arrived within {}ms", .after.as_millis())]
    Head {
        /// The window.
        after: Duration,
    },
    /// The whole-dispatch deadline elapsed.
    #[error("the dispatch deadline of {}ms elapsed", .after.as_millis())]
    TotalDeadline {
        /// The deadline.
        after: Duration,
    },
}

impl BudgetOverrun {
    /// Every overrun is a timeout or a protocol violation; none is a partial
    /// success.
    #[must_use]
    pub const fn kind(&self) -> crate::error::ProviderFailureKind {
        match self {
            Self::IdleFrame { .. }
            | Self::FirstFrame { .. }
            | Self::Head { .. }
            | Self::TotalDeadline { .. } => crate::error::ProviderFailureKind::Timeout,
            _ => crate::error::ProviderFailureKind::ProtocolViolation,
        }
    }
}

/// Running counters for one dispatch, checked as content accumulates.
#[derive(Debug, Default)]
pub struct BudgetLedger {
    /// Response bytes seen.
    pub response_bytes: u64,
    /// Frames decoded.
    pub frames: u32,
    /// Blocks opened.
    pub blocks: u16,
    /// Tool calls opened.
    pub tool_calls: u16,
    /// Assistant text bytes accumulated.
    pub text_bytes: u64,
    /// Reasoning bytes accumulated.
    pub reasoning_bytes: u64,
}

impl BudgetLedger {
    /// Charges response bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::Response`] when the whole-response bound is
    /// crossed.
    pub fn charge_response(
        &mut self,
        budget: &StreamBudget,
        bytes: u64,
    ) -> Result<(), BudgetOverrun> {
        self.response_bytes = self.response_bytes.saturating_add(bytes);
        if self.response_bytes > budget.max_response_bytes {
            return Err(BudgetOverrun::Response {
                limit: budget.max_response_bytes,
            });
        }
        Ok(())
    }

    /// Opens a block.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::Blocks`] when the block bound is crossed.
    pub fn open_block(&mut self, budget: &StreamBudget) -> Result<(), BudgetOverrun> {
        self.blocks = self.blocks.saturating_add(1);
        if self.blocks > budget.max_blocks {
            return Err(BudgetOverrun::Blocks {
                limit: budget.max_blocks,
            });
        }
        Ok(())
    }

    /// Opens a tool call.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::ToolCalls`] when the call bound is crossed.
    pub fn open_tool_call(&mut self, budget: &StreamBudget) -> Result<(), BudgetOverrun> {
        self.tool_calls = self.tool_calls.saturating_add(1);
        if self.tool_calls > budget.max_tool_calls {
            return Err(BudgetOverrun::ToolCalls {
                limit: budget.max_tool_calls,
            });
        }
        Ok(())
    }

    /// Charges assistant text.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::Text`] when the text bound is crossed.
    pub fn charge_text(&mut self, budget: &StreamBudget, bytes: u64) -> Result<(), BudgetOverrun> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > budget.max_text_bytes {
            return Err(BudgetOverrun::Text {
                limit: budget.max_text_bytes,
            });
        }
        Ok(())
    }

    /// Charges reasoning text.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::Reasoning`] when the reasoning bound is
    /// crossed.
    pub fn charge_reasoning(
        &mut self,
        budget: &StreamBudget,
        bytes: u64,
    ) -> Result<(), BudgetOverrun> {
        self.reasoning_bytes = self.reasoning_bytes.saturating_add(bytes);
        if self.reasoning_bytes > budget.max_reasoning_bytes {
            return Err(BudgetOverrun::Reasoning {
                limit: budget.max_reasoning_bytes,
            });
        }
        Ok(())
    }

    /// Checks one tool call's argument accumulation.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetOverrun::ToolArguments`] when the per-call bound is
    /// crossed.
    pub fn check_tool_arguments(
        budget: &StreamBudget,
        call: &ToolCallId,
        accumulated: usize,
    ) -> Result<(), BudgetOverrun> {
        if accumulated > budget.max_tool_argument_bytes as usize {
            return Err(BudgetOverrun::ToolArguments {
                call: call.clone(),
                limit: budget.max_tool_argument_bytes,
            });
        }
        Ok(())
    }

    /// Records a decoded frame.
    pub fn count_frame(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::primitives::ToolCallId;

    use super::{BudgetLedger, BudgetOverrun, StreamBudget};
    use crate::error::ProviderFailureKind;

    fn tight() -> StreamBudget {
        StreamBudget {
            max_response_bytes: 16,
            max_blocks: 2,
            max_tool_calls: 1,
            max_text_bytes: 8,
            max_reasoning_bytes: 8,
            max_tool_argument_bytes: 4,
            ..StreamBudget::default()
        }
    }

    #[test]
    fn the_response_bound_fires_on_the_byte_that_crosses_it() {
        let budget = tight();
        let mut ledger = BudgetLedger::default();
        ledger
            .charge_response(&budget, 16)
            .expect("exactly the bound fits");
        assert_eq!(
            ledger.charge_response(&budget, 1),
            Err(BudgetOverrun::Response { limit: 16 })
        );
    }

    #[test]
    fn the_block_bound_fires() {
        let budget = tight();
        let mut ledger = BudgetLedger::default();
        ledger.open_block(&budget).expect("first");
        ledger.open_block(&budget).expect("second");
        assert_eq!(
            ledger.open_block(&budget),
            Err(BudgetOverrun::Blocks { limit: 2 })
        );
    }

    #[test]
    fn the_tool_call_bound_fires() {
        let budget = tight();
        let mut ledger = BudgetLedger::default();
        ledger.open_tool_call(&budget).expect("first");
        assert_eq!(
            ledger.open_tool_call(&budget),
            Err(BudgetOverrun::ToolCalls { limit: 1 })
        );
    }

    #[test]
    fn the_text_and_reasoning_bounds_are_separate() {
        let budget = tight();
        let mut ledger = BudgetLedger::default();
        ledger.charge_text(&budget, 8).expect("text fits");
        ledger.charge_reasoning(&budget, 8).expect("reasoning fits");
        assert_eq!(
            ledger.charge_text(&budget, 1),
            Err(BudgetOverrun::Text { limit: 8 })
        );
    }

    #[test]
    fn the_tool_argument_bound_names_the_call() {
        let budget = tight();
        let call = ToolCallId::new("call_1").expect("id");
        BudgetLedger::check_tool_arguments(&budget, &call, 4).expect("exactly the bound fits");
        assert_eq!(
            BudgetLedger::check_tool_arguments(&budget, &call, 5),
            Err(BudgetOverrun::ToolArguments { call, limit: 4 })
        );
    }

    #[test]
    fn timeouts_classify_as_timeouts_and_byte_overruns_as_protocol_violations() {
        assert_eq!(
            BudgetOverrun::IdleFrame {
                after: core::time::Duration::from_secs(1)
            }
            .kind(),
            ProviderFailureKind::Timeout
        );
        assert_eq!(
            BudgetOverrun::Frame { limit: 1, seen: 2 }.kind(),
            ProviderFailureKind::ProtocolViolation
        );
    }

    #[test]
    fn a_saturating_charge_still_trips_the_bound() {
        let budget = tight();
        let mut ledger = BudgetLedger::default();
        assert!(ledger.charge_response(&budget, u64::MAX).is_err());
    }
}
