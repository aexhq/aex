//! rig client construction per dialect class.
//!
//! One client per dispatch, built over the shared workspace `reqwest::Client`
//! with the decrypted key borrowed by value. `DeepSeek`, `xAI`, and `Moonshot` use
//! their maintained Rig providers. `Meta` and `Alibaba` use Rig's generic
//! `OpenAI`-compatible chat-completions client with their compiled official
//! origins. `OpenAI` and `Anthropic` use native Rig providers. Aex owns no
//! provider wire implementation.

use aex_model_vocabulary::DialectClass;
use aex_wire::provider::ProviderId;
use rig_core::providers::{anthropic, deepseek, moonshot, openai, xai};

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
    /// Native `Anthropic` Messages.
    Anthropic(anthropic::Client),
    /// Native `DeepSeek` `OpenAI`-compatible transport.
    DeepSeek(deepseek::Client),
    /// Native `xAI` Responses transport.
    XAi(xai::Client),
    /// Native `Moonshot` `OpenAI`-compatible transport.
    Moonshot(moonshot::Client),
    /// Rig's maintained generic OpenAI-compatible chat-completions transport.
    OpenAiCompatible(openai::CompletionsClient),
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
            DialectClass::DeepSeekChat => Ok(Self::DeepSeek(
                deepseek::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds a deepseek client"),
            )),
            DialectClass::XAiResponses => Ok(Self::XAi(
                xai::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds an xai client"),
            )),
            DialectClass::MoonshotChat => Ok(Self::Moonshot(
                moonshot::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds a moonshot client"),
            )),
            DialectClass::MetaChat | DialectClass::AlibabaChat => Ok(Self::OpenAiCompatible(
                openai::Client::builder()
                    .api_key(key)
                    .base_url(url)
                    .http_client(http.clone())
                    .build()
                    .expect("a compiled origin builds an openai-compatible client")
                    .completions_api(),
            )),
        }
    }
}
