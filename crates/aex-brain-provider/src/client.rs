//! rig client construction per dialect class.
//!
//! One client per dispatch, built over the shared workspace `reqwest::Client`
//! with the decrypted key borrowed by value. The four OpenAI-compatible
//! dialects (`deepseek`, `zai`, `moonshotai`, `openrouter`, `vercel`) all ride
//! rig's OpenAI-chat-completions-compatible provider with a compiled
//! `base_url` override; `openai`, `anthropic` and `google` use their native
//! rig providers, whose auth headers are rig's own.

use aex_model_vocabulary::DialectClass;
use aex_wire::provider::ProviderId;
use rig_core::providers::{anthropic, deepseek, gemini, openai};

/// Why a compiled row cannot become a rig client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientBuildError {
    /// The compiled base URL is not a valid URL.
    #[error("compiled base url for `{provider}` is invalid: {detail}")]
    InvalidBaseUrl {
        /// The provider.
        provider: ProviderId,
        /// Why.
        detail: String,
    },
}

/// A dispatch-scoped rig client for one dialect class.
///
/// Constructed per dispatch so the decrypted key never outlives the send.
pub(crate) enum DispatchClient {
    /// Native `OpenAI` (Responses).
    OpenAi(openai::Client),
    /// Native Anthropic Messages.
    Anthropic(anthropic::Client),
    /// Native Gemini `streamGenerateContent`.
    Gemini(gemini::Client),
    /// The five OpenAI-compatible chat-completions dialects.
    Compatible(deepseek::Client),
}

impl DispatchClient {
    /// Builds the client for one compiled row, keyed with the decrypted key.
    ///
    /// # Errors
    ///
    /// Returns [`ClientBuildError::InvalidBaseUrl`] when the compiled origin is
    /// not a valid base URL, which can only be a generator/table regression.
    pub(crate) fn build(
        dialect: DialectClass,
        base_url: &str,
        key: &str,
        http: &reqwest::Client,
    ) -> Result<Self, ClientBuildError> {
        let provider = dialect.provider();
        let url = url::Url::parse(base_url).map_err(|error| ClientBuildError::InvalidBaseUrl {
            provider,
            detail: error.to_string(),
        })?;
        match dialect {
            DialectClass::OpenAiResponses => Ok(Self::OpenAi(
                openai::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds an openai client"),
            )),
            DialectClass::AnthropicMessages => Ok(Self::Anthropic(
                anthropic::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds an anthropic client"),
            )),
            DialectClass::GeminiGenerateContent => Ok(Self::Gemini(
                gemini::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds a gemini client"),
            )),
            DialectClass::DeepSeekChat
            | DialectClass::ZaiChat
            | DialectClass::MoonshotChat
            | DialectClass::OpenRouterChat
            | DialectClass::VercelAiGatewayChat => Ok(Self::Compatible(
                deepseek::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds a compatible client"),
            )),
        }
    }
}
