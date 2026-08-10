//! Provider response decoding and identity validation.

use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::{NormalizedEmail, Provider, ProviderAccountId};
use base64::Engine as _;
use serde::Deserialize;
use time::OffsetDateTime;

use super::HandshakeError;
use super::http::redact;

/// The two `iss` values Google signs an ID token with.
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

/// How far ahead of this process's clock a provider's `iat` may be.
const CLOCK_SKEW: Duration = Duration::from_mins(1);

/// The longest display name `identity.user` accepts.
const MAX_NAME_CHARS: usize = 128;

#[derive(Debug, Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    // GitHub's stable numeric identifier. A login is renameable.
    id: u64,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Debug, Deserialize)]
struct GoogleClaims {
    iss: String,
    aud: String,
    sub: String,
    exp: i64,
    iat: i64,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

/// Reads GitHub's token response.
///
/// GitHub reports a refused code with HTTP 200 and an `error` member, so the
/// successful-status body is still checked after all non-success statuses fail
/// closed.
pub(super) fn parse_github_token(
    status: reqwest::StatusCode,
    body: &[u8],
    secret: &str,
) -> Result<String, HandshakeError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(HandshakeError::RateLimited);
    }
    if status.is_client_error() {
        let detail = serde_json::from_slice::<GithubTokenResponse>(body)
            .ok()
            .and_then(|response| {
                response
                    .error
                    .map(|error| describe(&error, response.error_description.as_deref()))
            })
            .unwrap_or_else(|| format!("GitHub refused the token request with {status}"));
        return Err(HandshakeError::Refused(redact(&detail, secret)));
    }
    if !status.is_success() {
        return Err(HandshakeError::Unreachable(format!(
            "GitHub's token endpoint answered {status}"
        )));
    }
    let parsed: GithubTokenResponse = serde_json::from_slice(body).map_err(|_| {
        HandshakeError::Unreachable(format!(
            "GitHub answered {status} with a body that is not a token response"
        ))
    })?;
    if let Some(error) = parsed.error {
        return Err(HandshakeError::Refused(redact(
            &describe(&error, parsed.error_description.as_deref()),
            secret,
        )));
    }
    parsed
        .access_token
        .filter(|it| !it.is_empty())
        .ok_or_else(|| {
            HandshakeError::Unusable("GitHub answered without an access token".to_owned())
        })
}

/// Reads Google's token response, returning the ID token.
pub(super) fn parse_google_token(
    status: reqwest::StatusCode,
    body: &[u8],
    secret: &str,
) -> Result<String, HandshakeError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(HandshakeError::RateLimited);
    }
    if status.is_client_error() {
        let detail = serde_json::from_slice::<GoogleTokenResponse>(body)
            .ok()
            .and_then(|response| {
                response
                    .error
                    .map(|error| describe(&error, response.error_description.as_deref()))
            })
            .unwrap_or_else(|| format!("Google refused the token request with {status}"));
        return Err(HandshakeError::Refused(redact(&detail, secret)));
    }
    if !status.is_success() {
        return Err(HandshakeError::Unreachable(format!(
            "Google's token endpoint answered {status}"
        )));
    }
    let parsed: GoogleTokenResponse = serde_json::from_slice(body).map_err(|_| {
        HandshakeError::Unreachable(format!(
            "Google answered {status} with a body that is not a token response"
        ))
    })?;
    if let Some(error) = parsed.error {
        return Err(HandshakeError::Refused(redact(
            &describe(&error, parsed.error_description.as_deref()),
            secret,
        )));
    }
    parsed.id_token.filter(|it| !it.is_empty()).ok_or_else(|| {
        HandshakeError::Unusable(
            "Google answered without an ID token; the request did not ask for `openid`".to_owned(),
        )
    })
}

fn describe(error: &str, description: Option<&str>) -> String {
    description.map_or_else(|| error.to_owned(), |detail| format!("{error}: {detail}"))
}

/// Refuses a non-`2xx` GitHub profile read.
pub(super) fn check_read(
    status: reqwest::StatusCode,
    body: &[u8],
    token: &str,
) -> Result<(), HandshakeError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(HandshakeError::RateLimited);
    }
    if status.is_success() {
        return Ok(());
    }
    Err(HandshakeError::Unreachable(redact(
        &format!(
            "GitHub answered {status}: {}",
            String::from_utf8_lossy(body)
        ),
        token,
    )))
}

/// Builds a person from GitHub's two reads.
pub(super) fn github_profile(user: &[u8], emails: &[u8]) -> Result<OauthProfile, HandshakeError> {
    let user: GithubUser = serde_json::from_slice(user).map_err(|_| {
        HandshakeError::Unusable("GitHub's user is not the documented shape".to_owned())
    })?;
    let emails: Vec<GithubEmail> = serde_json::from_slice(emails).map_err(|_| {
        HandshakeError::Unusable("GitHub's email list is not the documented shape".to_owned())
    })?;
    let address = emails
        .iter()
        .find(|it| it.primary && it.verified)
        .ok_or_else(|| {
            HandshakeError::Unusable(
                "GitHub asserts no verified primary address for this account".to_owned(),
            )
        })?;
    profile(
        Provider::Github,
        &user.id.to_string(),
        &address.email,
        user.name,
        user.avatar_url,
    )
}

/// Builds a person from a Google ID token.
///
/// Every claim this platform acts on is checked: the issuer is Google, the
/// audience is our client, the token is inside its own validity window, and the
/// address is one Google asserts it has verified.
///
/// The signature is deliberately not checked. `OpenID` Connect Core §3.1.3.7
/// validation step 6 permits TLS server authentication when the ID token is
/// received by direct communication between the client and token endpoint.
/// Here that is a TLS-authenticated POST to the compiled, pinned
/// `oauth2.googleapis.com` endpoint; redirects and proxies are disabled. This
/// decision expires if an ID token can arrive from a browser or another
/// service, or if those transport constraints change; that change must add
/// local JWS validation before release. The durable rationale is also recorded
/// in `references/rewrite/clients.md`.
pub(super) fn google_profile(
    id_token: &str,
    client_id: &str,
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

/// Narrows a provider's answer onto the domain's own types.
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
