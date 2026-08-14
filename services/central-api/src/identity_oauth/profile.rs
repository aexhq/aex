//! Provider response decoding and identity normalization.

use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::{NormalizedEmail, Provider, ProviderAccountId};
use base64::Engine as _;
use serde::Deserialize;
use time::OffsetDateTime;

use super::HandshakeError;

/// The two `iss` values Google signs an ID token with.
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
/// How far ahead of this process's clock a provider's `iat` may be.
const CLOCK_SKEW: Duration = Duration::from_mins(1);
/// The longest display name `identity.user` accepts.
const MAX_NAME_CHARS: usize = 128;

#[derive(Debug, Deserialize)]
struct GoogleClaims {
    iss: String,
    aud: String,
    sub: String,
    exp: i64,
    iat: i64,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

/// Builds a person from the ID token received directly from Google's pinned
/// TLS token endpoint. Google documents direct, intermediary-free HTTPS plus
/// client authentication as sufficient provenance for this server flow. The
/// claims AEX acts on are still validated locally: issuer, audience, lifetime,
/// subject and verified email.
pub(super) fn google_profile(
    id_token: &str,
    client_id: &str,
    expected_nonce: &str,
    now: OffsetDateTime,
) -> Result<OauthProfile, HandshakeError> {
    let claims = google_claims(id_token)?;
    if !GOOGLE_ISSUERS.contains(&claims.iss.as_str()) {
        return Err(HandshakeError::Unusable(
            "the ID token was not issued by Google".to_owned(),
        ));
    }
    if claims.aud != client_id {
        return Err(HandshakeError::Unusable(
            "the ID token was issued to another OAuth client".to_owned(),
        ));
    }
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(HandshakeError::Unusable(
            "the ID token was not bound to this sign-in attempt".to_owned(),
        ));
    }
    let now_seconds = now.unix_timestamp();
    if claims.exp <= now_seconds {
        return Err(HandshakeError::Unusable(
            "the ID token has expired".to_owned(),
        ));
    }
    let skew = i64::try_from(CLOCK_SKEW.as_secs()).unwrap_or(60);
    if claims.iat > now_seconds.saturating_add(skew) {
        return Err(HandshakeError::Unusable(
            "the ID token was issued in the future".to_owned(),
        ));
    }
    if claims.email_verified != Some(true) {
        return Err(HandshakeError::Unusable(
            "Google does not assert this address is verified".to_owned(),
        ));
    }
    let email = claims
        .email
        .ok_or_else(|| HandshakeError::Unusable("the ID token carries no address".to_owned()))?;
    profile(
        Provider::Google,
        &claims.sub,
        &email,
        claims.name,
        claims.picture,
    )
}

fn google_claims(id_token: &str) -> Result<GoogleClaims, HandshakeError> {
    let mut segments = id_token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(HandshakeError::Unusable(
            "the ID token is not a three-segment JWT".to_owned(),
        ));
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| {
            HandshakeError::Unusable("the ID token's claims are not base64url".to_owned())
        })?;
    serde_json::from_slice(&decoded).map_err(|_| {
        HandshakeError::Unusable("the ID token's claims are not the documented shape".to_owned())
    })
}

fn profile(
    provider: Provider,
    account_id: &str,
    email: &str,
    name: Option<String>,
    image_url: Option<String>,
) -> Result<OauthProfile, HandshakeError> {
    let provider_account_id = ProviderAccountId::parse(account_id).map_err(|_| {
        HandshakeError::Unusable("the provider's account identifier is not storable".to_owned())
    })?;
    let email = NormalizedEmail::parse(email).map_err(|_| {
        HandshakeError::Unusable("the provider's address does not normalize".to_owned())
    })?;
    Ok(OauthProfile {
        provider,
        provider_account_id,
        email,
        email_verified: true,
        name: name.filter(|it| !it.trim().is_empty() && it.chars().count() <= MAX_NAME_CHARS),
        image_url: image_url
            .and_then(|url| aex_wire::types::HttpsUrl::parse(&url).ok())
            .map(|url| url.as_str().to_owned()),
    })
}
