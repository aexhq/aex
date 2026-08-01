//! `aex-brain-provider-gateway` owns the direct `BYOK` provider transport and dialect
//! adapter: six provider modules over one shared bounded `HTTP`/`SSE` core.
//!
//! # Invariants
//!
//! - there is no gateway, no arbitrary base `URL` and no cross-provider fallback
//! - a stream is bounded in frame size, total bytes and wall time before it is consumed
//! - an ambiguous disconnect is reported as ambiguous, never normalized into success or
//!   failure
//! - a provider module is structurally unable to see a credential: `WireRequest::auth`
//!   is a closed tag the shared core resolves into a sensitive header
//! - `DispatchProof::NotSent` is producible only while the `SendGate` is still held
//!
//! # Not this crate's job
//!
//! - choosing a model or routing policy (`aex-model-catalog`)
//! - token accounting as money: tokens are zero-dollar `BYOK` observability facts
//! - credential storage (`aex-secret-aws`, `aex-secret-custody-dynamodb`)

pub mod adapter;
pub mod anthropic;
pub mod budget;
pub mod credential;
pub mod deepseek;
pub mod error;
pub mod google;
pub mod moonshotai;
pub mod openai;
pub mod pool;
pub mod redact;
pub mod sse;
pub mod transport;
pub mod wire_pending;
pub mod zai;

pub use adapter::{ProviderAdapter, RequestBuildError};
pub use budget::{BudgetOverrun, StreamBudget};
pub use credential::{
    CredentialResolveError, DenyAllCredentialDecryptor, DenyAllCredentialDirectory, ProviderApiKey,
    ProviderCredentialDecryptor, ProviderCredentialDirectory,
};
pub use error::{ProviderFailureClass, ProviderFailureKind, RateLimitFeedback, RedactedDetail};
pub use sse::{SseDecoder, SseError, SseEvent};
pub use transport::{AuthScheme, Dispatched, SendGate, WireRequest};
pub use wire_pending::{DispatchProof, DispatchStage, ProviderPort};
