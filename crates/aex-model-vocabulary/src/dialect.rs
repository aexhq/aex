//! The compiled dialect an adapter speaks.
//!
//! Provider-neutral protocol vocabulary: the slim catalog's generated table
//! names one of these classes per admitted row, and a canonical receipt
//! records which class a dispatch used. There is exactly one class per launch
//! provider family.

use aex_wire::provider::ProviderId;
use serde::{Deserialize, Serialize};

/// The wire dialect class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialectClass {
    /// `OpenAI` Responses.
    OpenAiResponses,
    /// Anthropic Messages.
    AnthropicMessages,
    /// `DeepSeek` chat completions.
    DeepSeekChat,
    /// xAI Responses.
    XAiResponses,
    /// Meta's official OpenAI-compatible chat completions.
    MetaChat,
    /// `Moonshot` chat completions.
    MoonshotChat,
    /// Alibaba Model Studio's official OpenAI-compatible chat completions.
    AlibabaChat,
}

impl DialectClass {
    /// The provider that owns the dialect.
    #[must_use]
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::OpenAiResponses => ProviderId::Openai,
            Self::AnthropicMessages => ProviderId::Anthropic,
            Self::DeepSeekChat => ProviderId::Deepseek,
            Self::XAiResponses => ProviderId::Xai,
            Self::MetaChat => ProviderId::Meta,
            Self::MoonshotChat => ProviderId::Moonshotai,
            Self::AlibabaChat => ProviderId::Alibaba,
        }
    }

    /// The most tool declarations one dispatch may carry for this dialect.
    #[must_use]
    pub const fn max_tools(self) -> u16 {
        128
    }

    /// The smallest cacheable-prefix length the caller may declare.
    #[must_use]
    pub const fn min_cacheable_prefix_tokens(self) -> u32 {
        1_024
    }

    /// Whether a sealed assistant turn must carry reasoning round-trip material.
    #[must_use]
    pub const fn requires_reasoning_token(self, has_tool_use: bool) -> bool {
        // Anthropic thinking blocks must round-trip on tool-use turns.
        matches!(self, Self::AnthropicMessages) && has_tool_use
    }
}
