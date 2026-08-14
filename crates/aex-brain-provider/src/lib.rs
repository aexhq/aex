//! `aex-brain-provider` implements the Brain `ProviderPort` over rig:
//! dialect routing onto the compiled models.dev admit table, bounded pre-send
//! retry, canonical frame mapping, and receipt sealing.
//!
//! # Invariants
//!
//! - a request is sent only for a `(provider, model)` pair the compiled latest
//!   catalog admits; Aex exposes no third-party gateway provider identity
//! - the retry loop re-sends only after a definitive `429`/`503` rejection; an
//!   ambiguous or started send is never followed by a second generation
//! - an exhausted retry loop stops deterministically (`Terminal`), so the
//!   Brain settles a `KnownFailure` and the run ends
//! - the decrypted key exists only inside rig's client for the duration of
//!   one dispatch; the cache shares one zeroizing allocation per binding
//!
//! # Not this crate's job
//!
//! - model admission data (`aex-model-catalog`)
//! - credential storage (`aex-brain-provider-custody`)
//! - the canonical vocabulary (`aex-model-vocabulary`)

#![forbid(unsafe_code)]

mod cache;
mod client;
mod credential_flight;
mod request;
mod response;
mod router;

pub use cache::{CACHE_CAPACITY, CACHE_TTL, CredentialCache, CredentialCacheKey};
pub use router::{ProviderRouterBuildError, RigProviderRouter};

#[doc(hidden)]
pub use aex_model_catalog::canonical::CanonicalModelRequest;

#[doc(hidden)]
pub use request::RequestBuildError;

/// Test-only: the full rig request for one canonical request, without
/// dispatching. Hidden from docs on purpose.
#[doc(hidden)]
pub fn translate_request(
    request: &CanonicalModelRequest,
) -> Result<rig_core::completion::CompletionRequest, RequestBuildError> {
    crate::request::build(request, request.selection.dialect())
}
