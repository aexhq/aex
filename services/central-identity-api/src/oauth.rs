//! The provider handshake, performed here rather than asserted by a caller.
//!
//! # What this replaces, and why
//!
//! `dashboard_session_create` used to accept an already-established provider
//! profile and prove the caller was a first-party front end with a shared
//! secret. That design generalized a local Stripe-gateway rule into “Rust never
//! talks to a third-party vendor”, even though provider gateways elsewhere in
//! the tree already dial their vendors directly.
//!
//! A language is not a security boundary. The actual control is which
//! deployable holds which credential, declared in a capability manifest and
//! bound to a named secret. This crate declares and binds
//! [`aex_central_http::capability::SignInHandshake`], without which the process
//! refuses to start. The code is therefore redeemed here and an untrusted
//! caller never asserts an identity; the obsolete shared secret has nothing
//! left to prove.
//!
//! # Outbound HTTP policy
//!
//! [`http`] follows the established provider-gateway transport policy:
//!
//! * no proxy, redirects, or cookie store; rustls and HTTPS-only transport;
//! * compiled, host-pinned provider endpoints;
//! * credentials attached at one send path and redacted from diagnostics;
//! * response bodies read against a fixed size ceiling.
//!
//! It deliberately does not retry. An authorization code is single-use: after a
//! request may have reached the token endpoint, the code may already be spent.
//! A failure is reported rather than re-sent.

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::Provider;
use subtle::ConstantTimeEq as _;

mod client;
mod http;
mod profile;

pub use client::{OauthClient, OauthClientError, load_oauth_client};
pub use http::{ClientBuildError, HttpProviderHandshake};

#[cfg(test)]
use http::{Endpoints, redact};
#[cfg(test)]
use profile::{github_profile, google_profile};

/// Why the handshake did not produce a person.
///
/// “The provider refused your code” and “the provider did not answer” remain
/// different outcomes: the first is the caller's problem and the second is an
/// upstream availability failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// The provider refused the code: it never existed, was already redeemed,
    /// expired, or was issued to another client.
    #[error("the provider refused the authorization code: {0}")]
    Refused(String),
    /// The provider answered with something this platform cannot make a person
    /// from: an unparsable body, invalid claim, or no verified address.
    #[error("the provider's answer is not a usable identity: {0}")]
    Unusable(String),
    /// The provider rate-limited the exchange.
    #[error("the provider rate-limited the exchange")]
    RateLimited,
    /// The exchange never completed. The code may or may not have been spent,
    /// which is why this result is never retried.
    #[error("the provider handshake did not complete: {0}")]
    Unreachable(String),
}

/// The RFC 7636 S256 challenge of a verifier.
///
/// `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))`, unpadded, which is always
/// 43 characters.
#[must_use]
pub fn challenge_of(verifier: &str) -> String {
    oauth2::PkceCodeChallenge::from_code_verifier_sha256(&oauth2::PkceCodeVerifier::new(
        verifier.to_owned(),
    ))
    .as_str()
    .to_owned()
}

/// Whether the `state` a redirect carried is the challenge of this verifier.
///
/// # Why this is the whole CSRF story
///
/// A sign-in that can be cross-site-forged authenticates the wrong person. An
/// attacker can start their own provider sign-in and try to make a victim's
/// browser complete it, leaving the victim with a session for the attacker's
/// account.
///
/// The verifier is minted by the front end and kept in an origin-scoped,
/// `HttpOnly` cookie. Only its S256 challenge travels, as both PKCE
/// `code_challenge` and `state`. An attacker can mint their own pair, but cannot
/// make the victim's browser present the attacker's verifier without writing a
/// cookie on an origin they do not control. A browser that never started a
/// sign-in has no verifier and is refused before any provider is dialled.
///
/// The comparison is constant-time. The challenge is public, so this is
/// belt-and-braces rather than load-bearing, and it costs nothing.
#[must_use]
pub fn state_matches(verifier: &str, state: &str) -> bool {
    let expected = challenge_of(verifier);
    expected.as_bytes().ct_eq(state.as_bytes()).into()
}

/// The provider handshake, as a port.
///
/// `AuthService` depends on this rather than on [`HttpProviderHandshake`], so
/// composition tests can drive the ceremony without a network.
#[async_trait::async_trait]
pub trait ProviderHandshake: Send + Sync + std::fmt::Debug {
    /// Redeems a single-use authorization code and reads who authorized it.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeError`] naming what the provider did.
    async fn identify(
        &self,
        provider: Provider,
        code: &str,
        verifier: &str,
    ) -> Result<OauthProfile, HandshakeError>;
}

#[cfg(test)]
#[path = "oauth/tests.rs"]
mod tests;
