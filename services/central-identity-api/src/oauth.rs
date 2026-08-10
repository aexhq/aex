//! The provider handshake, performed here rather than asserted by a caller.
//!
//! # What this replaces, and why
//!
//! `dashboard_session_create` used to accept an *already-established* provider
//! profile — provider, account id, address, verified flag — and prove the caller
//! was a first-party front end with a shared secret. The rule it was built on
//! was that "Rust never talks to a third-party vendor", generalised from one
//! sentence in `finance-api`'s Stripe gateway.
//!
//! The tree contradicts that generalisation four times over:
//! `crates/aex-brain-provider-gateway` dials Anthropic, `OpenAI`, Google and
//! `DeepSeek` directly, holding each customer's credential, and that is the core
//! of the product. A language is not a security boundary. The control that does
//! the work is *which deployable holds which credential*, declared in a
//! capability manifest and bound to a named secret — see
//! [`aex_central_http::capability::SignInHandshake`], which this crate declares
//! and binds, and without which the process refuses to start.
//!
//! So the exchange happens here. There is no untrusted caller asserting an
//! identity any more, which is why the shared secret is gone rather than
//! reduced: with the code redeemed server-side there is nothing left for it to
//! prove.
//!
//! # Following the outbound-HTTP pattern that already exists
//!
//! `aex-brain-provider-gateway`'s transport is the house pattern and this module
//! follows it rather than inventing a second one:
//!
//! * the same client policy — no proxy, **no redirect following**, no cookie
//!   store, rustls, `https_only`, a named user agent. A redirect from a pinned
//!   provider origin is an incident, never a hop to chase;
//! * compiled origins. Every endpoint is a constant in this module, asserted
//!   `https` and host-pinned by [`tests`], so no caller-supplied string can
//!   choose who we hand a client secret to;
//! * the credential never appears in a request this module can render. It is
//!   attached at the single send site and every provider-derived diagnostic goes
//!   through [`redact`] before it can reach an error;
//! * bounded response bodies, read chunk by chunk against a ceiling rather than
//!   buffered on the provider's word.
//!
//! The one place it deliberately *differs* is retry. The gateway retries under a
//! catalog policy because a model call is repeatable. An OAuth authorization
//! code is single-use: once a request may have reached the token endpoint the
//! code may already be spent, so a retry either fails for certain or redeems a
//! grant twice. Every call here is made exactly once, and a failure after the
//! request left the process is reported as a failure rather than re-sent.

use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::{NormalizedEmail, Provider, ProviderAccountId};
use base64::Engine as _;
use serde::Deserialize;
use sha2::Digest as _;
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;

/// GitHub's authorization-code redemption endpoint.
const GITHUB_TOKEN_ENDPOINT: &str = "https://github.com/login/oauth/access_token";
/// GitHub's REST origin, where the redeemed token reads the person.
const GITHUB_API_ORIGIN: &str = "https://api.github.com";
/// Google's authorization-code redemption endpoint.
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The pinned GitHub REST API version.
///
/// Date-versioned and pinned for the same reason the brain gateway pins
/// `anthropic-version`: an unpinned client silently follows whatever the vendor
/// makes current, and a response shape that changes underneath a running plane
/// is a failure nobody deployed. `2026-03-10` is the current version; the
/// previous one is supported until 2028-03-10, so this pin has a runway.
const GITHUB_API_VERSION: &str = "2026-03-10";

/// The two `iss` values Google signs an ID token with.
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

/// How far ahead of this process's clock a provider's `iat` may be.
const CLOCK_SKEW: Duration = Duration::from_mins(1);

/// Time to establish a connection to a provider.
///
/// The same three seconds `aex_brain_provider_gateway::budget::DEFAULT_CONNECT_TIMEOUT`
/// allows, and for the same reason: a provider that has not completed a
/// handshake in three seconds is not about to.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The largest provider response this module will read.
///
/// A token response is a few hundred bytes and a GitHub user is a few thousand.
/// The bound is enforced while reading rather than from `Content-Length`, so a
/// provider that under-reports cannot make this process buffer without limit.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// The longest display name `identity.user` accepts.
///
/// `user_name_len_ck` is `char_length(name) BETWEEN 1 AND 128`. A longer one is
/// dropped rather than truncated or refused: the name is cosmetic, refusing the
/// sign-in over it would lock a person out of the platform for the length of
/// their own name, and a truncation would write a value the provider never
/// asserted.
const MAX_NAME_CHARS: usize = 128;

/// Why the handshake did not produce a person.
///
/// Each arm is a different answer to the caller, and the split is the point:
/// "the provider refused your code" and "the provider did not answer" must not
/// collapse, because the first is the caller's problem and the second is ours.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// The provider refused the code: it never existed, it was already
    /// redeemed, it expired, or it was issued to another client.
    #[error("the provider refused the authorization code: {0}")]
    Refused(String),
    /// The provider answered with something this platform cannot make a person
    /// from — an unparsable body, a claim that does not check out, no verified
    /// address.
    #[error("the provider's answer is not a usable identity: {0}")]
    Unusable(String),
    /// The provider rate-limited the exchange.
    #[error("the provider rate-limited the exchange")]
    RateLimited,
    /// The exchange never completed. The code may or may not have been spent,
    /// which is exactly why it is not retried.
    #[error("the provider handshake did not complete: {0}")]
    Unreachable(String),
}

/// One provider's registered OAuth client.
///
/// The secret is held as a `String` because a token endpoint needs it in
/// plaintext at every exchange, and it is kept out of every rendering this type
/// can produce.
#[derive(Clone, PartialEq, Eq)]
pub struct OauthClient {
    id: String,
    secret: String,
}

impl std::fmt::Debug for OauthClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OauthClient")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Why a configured OAuth client was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OauthClientError {
    /// The client id was absent or blank.
    #[error("the OAuth client id is blank")]
    BlankId,
    /// The client secret was absent or blank.
    #[error("the OAuth client secret is blank")]
    BlankSecret,
}

impl OauthClient {
    /// Validates a configured client.
    ///
    /// # Errors
    ///
    /// Returns [`OauthClientError`] when either half is blank. A blank secret
    /// would produce an exchange that fails on every sign-in for the life of
    /// the process, and start-up is the only place refusing it costs nothing.
    pub fn new(id: &str, secret: &str) -> Result<Self, OauthClientError> {
        let id = id.trim();
        let secret = secret.trim();
        if id.is_empty() {
            return Err(OauthClientError::BlankId);
        }
        if secret.is_empty() {
            return Err(OauthClientError::BlankSecret);
        }
        Ok(Self {
            id: id.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The public client identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The registered client for every provider this platform signs people in with.
///
/// One field per provider rather than a map: a map can be missing a key at
/// runtime, and this way a provider added to [`Provider`] does not compile until
/// somebody decides what client it uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OauthClients {
    github: OauthClient,
    google: OauthClient,
}

impl OauthClients {
    /// Holds the two registered clients.
    #[must_use]
    pub const fn new(github: OauthClient, google: OauthClient) -> Self {
        Self { github, google }
    }

    /// The client one provider's exchange authenticates as.
    #[must_use]
    pub const fn of(&self, provider: Provider) -> &OauthClient {
        match provider {
            Provider::Github => &self.github,
            Provider::Google => &self.google,
        }
    }
}

/// The shape a provider's OAuth client secret is stored in.
///
/// A JSON envelope, unlike the peppers this crate loads whole, because there are
/// genuinely two values and Secrets Manager stores a key/value secret natively.
/// One rotation writes both halves or neither.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredClient {
    client_id: String,
    client_secret: String,
}

/// Reads one provider's registered OAuth client from Secrets Manager.
///
/// # Errors
///
/// Returns the reason as a string, for the caller to name its own dependency
/// with. Nothing in the message can carry the secret: a parse failure names the
/// shape, never the value.
pub async fn load_oauth_client(
    secrets: &aws_sdk_secretsmanager::Client,
    secret_id: &str,
) -> Result<OauthClient, String> {
    let value = secrets
        .get_secret_value()
        .secret_id(secret_id)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let plaintext = value
        .secret_string()
        .ok_or_else(|| "the secret holds no string value".to_owned())?;
    let stored: StoredClient = serde_json::from_str(plaintext).map_err(|_| {
        "the secret is not a JSON object with `clientId` and `clientSecret`".to_owned()
    })?;
    OauthClient::new(&stored.client_id, &stored.client_secret).map_err(|error| error.to_string())
}

/// The RFC 7636 S256 challenge of a verifier.
///
/// `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))`, unpadded, which is always
/// 43 characters.
#[must_use]
pub fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// Whether the `state` a redirect carried is the challenge of this verifier.
///
/// # Why this is the whole CSRF story
///
/// A sign-in that can be cross-site-forged authenticates the wrong person, which
/// is a correctness defect and not a security preference. The classic forgery is
/// for an attacker to start their *own* provider sign-in, obtain a valid code,
/// and then make a victim's browser complete the callback — leaving the victim
/// holding a session for the attacker's account, and everything they subsequently
/// type inside it.
///
/// What stops that is binding the redirect to the browser that began the flow.
/// The verifier is minted by the front end, kept in a cookie for its own origin
/// that is `HttpOnly` and never leaves it, and only its S256 challenge travels —
/// as both the PKCE `code_challenge` and the `state`, which are the same value
/// by construction so there is exactly one secret to lose.
///
/// An attacker can mint their own pair, but to spend it they would have to make
/// the victim's browser present the *attacker's* verifier, which means writing a
/// cookie on an origin they do not control. The victim's own cookie hashes to
/// the victim's `state`, not the attacker's, and this comparison refuses it —
/// before any provider is dialled and before any code is spent. A browser that
/// never started a sign-in has no verifier at all and is refused for want of
/// one.
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
/// the composition suite can drive the whole ceremony over HTTP without a
/// network — the same way it drives the identity store in memory.
#[async_trait::async_trait]
pub trait ProviderHandshake: Send + Sync + std::fmt::Debug {
    /// Redeems a single-use authorization code and reads back who authorized it.
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

/// Where each provider is reached.
///
/// Compiled constants in production. The overriding constructor exists only for
/// this crate's own tests, which is what keeps a caller-supplied string from
/// ever choosing who receives a client secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    github_token: String,
    github_api: String,
    google_token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            github_token: GITHUB_TOKEN_ENDPOINT.to_owned(),
            github_api: GITHUB_API_ORIGIN.to_owned(),
            google_token: GOOGLE_TOKEN_ENDPOINT.to_owned(),
        }
    }
}

impl Endpoints {
    /// Every compiled origin, for the pinning assertions.
    #[must_use]
    pub fn all(&self) -> [&str; 3] {
        [&self.github_token, &self.github_api, &self.google_token]
    }
}

/// The handshake over HTTPS.
#[derive(Debug)]
pub struct HttpProviderHandshake {
    http: reqwest::Client,
    clients: OauthClients,
    redirect_uri: String,
    endpoints: Endpoints,
    deadline: Duration,
}

/// Why the handshake client could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("reqwest refused the pinned client configuration")]
pub struct ClientBuildError;

impl HttpProviderHandshake {
    /// Builds the handshake against the compiled provider origins.
    ///
    /// `deadline` bounds the **whole** handshake rather than each call inside
    /// it, so a sign-in can never outlive the request that asked for it however
    /// many round trips a provider needs. GitHub needs three; Google needs one.
    ///
    /// # Errors
    ///
    /// Returns [`ClientBuildError`] when `reqwest` refuses the pinned policy,
    /// which a fixed configuration cannot normally produce.
    pub fn new(
        clients: OauthClients,
        redirect_uri: String,
        deadline: Duration,
    ) -> Result<Self, ClientBuildError> {
        Self::with_endpoints(clients, redirect_uri, deadline, Endpoints::default())
    }

    fn with_endpoints(
        clients: OauthClients,
        redirect_uri: String,
        deadline: Duration,
        endpoints: Endpoints,
    ) -> Result<Self, ClientBuildError> {
        // The same refusals `aex_brain_provider_gateway::pool::build_client`
        // makes, for the same reasons: a redirect away from a pinned provider
        // origin is an incident rather than a hop to chase, and a proxy or a
        // cookie jar on a path that carries a client secret is a leak waiting
        // for a misconfiguration.
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .use_rustls_tls()
            .user_agent(concat!(
                "aex-central-identity-api/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|_| ClientBuildError)?;
        Ok(Self {
            http,
            clients,
            redirect_uri,
            endpoints,
            deadline,
        })
    }

    /// The single site in this module that hands a request to the network.
    ///
    /// Called at most once per provider round trip and never re-entered on
    /// failure: an authorization code that may already be spent cannot be
    /// presented again.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        secret: &str,
    ) -> Result<(reqwest::StatusCode, Vec<u8>), HandshakeError> {
        let response = request
            .send()
            .await
            .map_err(|error| HandshakeError::Unreachable(redact(&error.to_string(), secret)))?;
        let status = response.status();
        let body = bounded_body(response, secret).await?;
        Ok((status, body))
    }

    async fn github(&self, code: &str, verifier: &str) -> Result<OauthProfile, HandshakeError> {
        let client = self.clients.of(Provider::Github);
        let form = form(&[
            ("client_id", client.id()),
            ("client_secret", &client.secret),
            ("code", code),
            ("redirect_uri", &self.redirect_uri),
            ("code_verifier", verifier),
        ]);
        let (status, body) = self
            .send(
                self.http
                    .post(&self.endpoints.github_token)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(form),
                &client.secret,
            )
            .await?;
        let token = parse_github_token(status, &body, &client.secret)?;

        let (status, user) = self.send(self.github_read(&token, "/user"), &token).await?;
        check_read(status, &user, &token)?;
        let (status, emails) = self
            .send(self.github_read(&token, "/user/emails"), &token)
            .await?;
        check_read(status, &emails, &token)?;
        github_profile(&user, &emails)
    }

    fn github_read(&self, token: &str, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.endpoints.github_api))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("x-github-api-version", GITHUB_API_VERSION)
            .bearer_auth(token)
    }

    async fn google(
        &self,
        code: &str,
        verifier: &str,
        now: OffsetDateTime,
    ) -> Result<OauthProfile, HandshakeError> {
        let client = self.clients.of(Provider::Google);
        let form = form(&[
            ("client_id", client.id()),
            ("client_secret", &client.secret),
            ("code", code),
            ("redirect_uri", &self.redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
        ]);
        let (status, body) = self
            .send(
                self.http
                    .post(&self.endpoints.google_token)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(form),
                &client.secret,
            )
            .await?;
        let id_token = parse_google_token(status, &body, &client.secret)?;
        google_profile(&id_token, client.id(), now)
    }
}

#[async_trait::async_trait]
impl ProviderHandshake for HttpProviderHandshake {
    async fn identify(
        &self,
        provider: Provider,
        code: &str,
        verifier: &str,
    ) -> Result<OauthProfile, HandshakeError> {
        let now = OffsetDateTime::now_utc();
        let exchange = async {
            match provider {
                Provider::Github => self.github(code, verifier).await,
                Provider::Google => self.google(code, verifier, now).await,
            }
        };
        tokio::time::timeout(self.deadline, exchange)
            .await
            .map_err(|_| {
                HandshakeError::Unreachable(
                    "the provider did not answer inside this request's deadline".to_owned(),
                )
            })?
    }
}

/// Encodes an `application/x-www-form-urlencoded` body.
///
/// Built here rather than through a serializer so the exact bytes that carry a
/// client secret are visible at the one place they are assembled.
fn form(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

/// Reads a response body against [`MAX_RESPONSE_BYTES`], chunk by chunk.
async fn bounded_body(
    mut response: reqwest::Response,
    secret: &str,
) -> Result<Vec<u8>, HandshakeError> {
    let mut buffer = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|error| HandshakeError::Unreachable(redact(&error.to_string(), secret)))?;
        let Some(chunk) = chunk else { break };
        if buffer.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(HandshakeError::Unusable(format!(
                "the provider answered with more than {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

/// Replaces a secret and every long credential-shaped run in a diagnostic.
///
/// The same positive rule `aex_brain_provider_gateway::redact` applies: a
/// provider that echoes a client secret or an access token into an error body
/// must not be able to put it in this process's logs.
fn redact(text: &str, secret: &str) -> String {
    let bounded: String = text.chars().take(256).collect();
    let replaced = if secret.is_empty() {
        bounded
    } else {
        bounded.replace(secret, "[redacted]")
    };
    mask_runs(&replaced)
}

/// The shortest run of credential-shaped characters that is masked.
const MIN_SECRET_RUN: usize = 20;

fn mask_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            run.push(character);
            continue;
        }
        flush(&mut run, &mut out);
        out.push(character);
    }
    flush(&mut run, &mut out);
    out
}

fn flush(run: &mut String, out: &mut String) {
    if run.chars().count() >= MIN_SECRET_RUN {
        out.push_str("[redacted]");
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// What GitHub answers a code redemption with.
///
/// GitHub reports a refused code with **HTTP 200** and an `error` member, so the
/// status alone decides nothing here — reading the body is the only way to tell
/// a redeemed code from a rejected one.
#[derive(Debug, Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// What Google answers a code redemption with.
#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// One GitHub user.
#[derive(Debug, Deserialize)]
struct GithubUser {
    /// GitHub's stable numeric identifier. The login is *not* used: it is
    /// renameable, and a renamed login would resolve to a different person.
    id: u64,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// One entry of `GET /user/emails`.
#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

/// The claims this platform reads from a Google ID token.
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
fn parse_github_token(
    status: reqwest::StatusCode,
    body: &[u8],
    secret: &str,
) -> Result<String, HandshakeError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(HandshakeError::RateLimited);
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
fn parse_google_token(
    status: reqwest::StatusCode,
    body: &[u8],
    secret: &str,
) -> Result<String, HandshakeError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(HandshakeError::RateLimited);
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

/// Refuses a non-`2xx` read.
fn check_read(status: reqwest::StatusCode, body: &[u8], token: &str) -> Result<(), HandshakeError> {
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
fn github_profile(user: &[u8], emails: &[u8]) -> Result<OauthProfile, HandshakeError> {
    let user: GithubUser = serde_json::from_slice(user).map_err(|_| {
        HandshakeError::Unusable("GitHub's user is not the documented shape".to_owned())
    })?;
    let emails: Vec<GithubEmail> = serde_json::from_slice(emails).map_err(|_| {
        HandshakeError::Unusable("GitHub's email list is not the documented shape".to_owned())
    })?;
    // Primary and verified, in that order, and never merely verified: a person
    // with several verified addresses must resolve to one deterministic row.
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
/// # What is verified, and what is not
///
/// Every claim this platform acts on is checked: the issuer is Google, the
/// audience is *our* client, the token is inside its own validity window, and
/// the address is one Google asserts it has verified.
///
/// The **signature** is deliberately not checked, and this is the standard's own
/// carve-out rather than a shortcut. `OpenID` Connect Core §3.1.3.7 clause 2: when
/// the ID token is received by direct communication between the client and the
/// token endpoint — which is exactly this code path, a TLS-authenticated `POST`
/// to a pinned `oauth2.googleapis.com` origin that no browser touches — TLS
/// server authentication may be used to validate the issuer in place of checking
/// the token signature. The token never travels through the user agent, so there
/// is no channel on which a forged one could reach us: an attacker able to
/// substitute it has already broken TLS to a pinned host, at which point a
/// signature check over keys fetched from the same host proves nothing.
///
/// Checking it anyway would mean fetching and caching Google's JWKS, tracking
/// its rotation, and adding an RSA verifier to a workspace that has none — new
/// network dependency and new cryptography in exchange for no attacker removed.
/// If an ID token ever reaches this platform by any other route, that reasoning
/// expires with it and the signature must be verified.
fn google_profile(
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

/// Decodes a JWT's claim segment.
///
/// Structure only — three segments, the middle one base64url with no padding.
/// What the claims *mean* is [`google_profile`]'s decision.
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
///
/// This is where a provider stops being trusted about *shape*: the address must
/// normalize, the account id must be storable, the avatar must be an `https`
/// URL — a provider that answered `javascript:…` would otherwise have written it
/// into a field the dashboard renders.
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
        // Every path that reaches here has already refused an unverified
        // address. The flag stays in the struct because the store writes
        // `email_verified_at` from it.
        email_verified: true,
        name: name.filter(|it| !it.trim().is_empty() && it.chars().count() <= MAX_NAME_CHARS),
        image_url: image_url
            .and_then(|url| aex_wire::types::HttpsUrl::parse(&url).ok())
            .map(|url| url.as_str().to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Endpoints, GITHUB_API_VERSION, HandshakeError, HttpProviderHandshake, OauthClient,
        OauthClientError, OauthClients, challenge_of, form, github_profile, google_profile,
        parse_github_token, parse_google_token, redact, state_matches,
    };
    use aex_identity_domain::Provider;
    use base64::Engine as _;
    use std::time::Duration;
    use time::OffsetDateTime;

    fn clients() -> OauthClients {
        OauthClients::new(
            OauthClient::new("gh-client", "gh-secret").expect("a client"),
            OauthClient::new("goog-client", "goog-secret").expect("a client"),
        )
    }

    fn id_token(claims: &serde_json::Value) -> String {
        let encode = |value: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(value).expect("serializable"))
        };
        format!(
            "{}.{}.{}",
            encode(&serde_json::json!({"alg": "RS256"})),
            encode(claims),
            "c2lnbmF0dXJl"
        )
    }

    fn google_fixture(now: OffsetDateTime) -> serde_json::Value {
        serde_json::json!({
            "iss": "https://accounts.google.com",
            "aud": "goog-client",
            "sub": "1234567890",
            "exp": now.unix_timestamp() + 300,
            "iat": now.unix_timestamp() - 5,
            "email": "Person@Example.COM",
            "email_verified": true,
            "name": "A Person",
            "picture": "https://example.com/a.png",
        })
    }

    #[test]
    fn a_challenge_is_the_rfc_7636_s256_of_its_verifier() {
        // RFC 7636 appendix B's vector.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_state_matches_only_the_verifier_it_was_derived_from() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(state_matches(verifier, &challenge_of(verifier)));
        for forged in [
            "",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM ",
            "e9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            &challenge_of("another-verifier-entirely-0000000000000000000"),
        ] {
            assert!(!state_matches(verifier, forged), "{forged}");
        }
    }

    /// The forgery the `state` check exists to refuse: an attacker's own valid
    /// pair, presented against the verifier a victim's browser is holding.
    #[test]
    fn an_attackers_state_never_matches_a_victims_verifier() {
        let victim = "victim-verifier-0000000000000000000000000000";
        let attacker = "attacker-verifier-00000000000000000000000000";
        assert!(!state_matches(victim, &challenge_of(attacker)));
    }

    #[test]
    fn every_compiled_provider_origin_is_https_and_host_pinned() {
        let endpoints = Endpoints::default();
        for origin in endpoints.all() {
            assert!(origin.starts_with("https://"), "{origin}");
            let parsed = url::Url::parse(origin).expect("a compiled origin parses");
            assert_eq!(parsed.scheme(), "https");
            assert_eq!(parsed.port(), None, "{origin} pins a non-default port");
        }
        assert!(endpoints.all().iter().any(|it| it.contains("github.com")));
        assert!(
            endpoints
                .all()
                .iter()
                .any(|it| it.contains("oauth2.googleapis.com"))
        );
    }

    #[test]
    fn a_client_with_a_blank_half_is_refused_at_construction() {
        assert_eq!(
            OauthClient::new(" ", "secret"),
            Err(OauthClientError::BlankId)
        );
        assert_eq!(
            OauthClient::new("id", "  "),
            Err(OauthClientError::BlankSecret)
        );
    }

    #[test]
    fn a_client_never_renders_its_secret() {
        let rendered = format!("{:?}", clients());
        assert!(rendered.contains("gh-client"), "{rendered}");
        assert!(!rendered.contains("gh-secret"), "{rendered}");
        assert!(!rendered.contains("goog-secret"), "{rendered}");
    }

    #[test]
    fn each_provider_exchanges_as_its_own_registered_client() {
        let clients = clients();
        assert_eq!(clients.of(Provider::Github).id(), "gh-client");
        assert_eq!(clients.of(Provider::Google).id(), "goog-client");
    }

    #[test]
    fn a_form_body_percent_encodes_every_value() {
        assert_eq!(
            form(&[("code", "a b&c"), ("redirect_uri", "https://x.dev/cb")]),
            "code=a+b%26c&redirect_uri=https%3A%2F%2Fx.dev%2Fcb"
        );
    }

    /// GitHub reports a refused code with `200 OK` and an `error` member, so a
    /// status-only reading would mint a session from a rejected redemption.
    #[test]
    fn a_github_refusal_arrives_as_two_hundred_and_is_still_a_refusal() {
        let error = parse_github_token(
            reqwest::StatusCode::OK,
            br#"{"error":"bad_verification_code","error_description":"The code passed is incorrect or expired."}"#,
            "gh-secret",
        )
        .expect_err("a refusal");
        assert!(matches!(error, HandshakeError::Refused(_)), "{error}");
    }

    #[test]
    fn a_github_token_is_read_from_a_successful_redemption() {
        let token = parse_github_token(
            reqwest::StatusCode::OK,
            br#"{"access_token":"gho_fixture","scope":"user:email","token_type":"bearer"}"#,
            "gh-secret",
        )
        .expect("a token");
        assert_eq!(token, "gho_fixture");
    }

    #[test]
    fn a_provider_rate_limit_is_never_reported_as_a_refused_code() {
        for parsed in [
            parse_github_token(reqwest::StatusCode::TOO_MANY_REQUESTS, b"{}", "gh-secret"),
            parse_google_token(reqwest::StatusCode::TOO_MANY_REQUESTS, b"{}", "goog-secret"),
        ] {
            assert_eq!(parsed.expect_err("a refusal"), HandshakeError::RateLimited);
        }
    }

    #[test]
    fn a_google_refusal_is_read_from_its_error_member() {
        let error = parse_google_token(
            reqwest::StatusCode::BAD_REQUEST,
            br#"{"error":"invalid_grant","error_description":"Bad Request"}"#,
            "goog-secret",
        )
        .expect_err("a refusal");
        assert!(matches!(error, HandshakeError::Refused(_)), "{error}");
    }

    #[test]
    fn a_github_person_is_built_from_the_verified_primary_address() {
        let profile = github_profile(
            br#"{"id":42,"login":"person","name":"A Person","avatar_url":"https://example.com/a.png"}"#,
            br#"[{"email":"other@example.com","primary":false,"verified":true},
                 {"email":"Person@Example.COM","primary":true,"verified":true}]"#,
        )
        .expect("a person");
        assert_eq!(profile.provider, Provider::Github);
        // The numeric id, never the renameable login.
        assert_eq!(profile.provider_account_id.as_str(), "42");
        assert_eq!(profile.email.as_str(), "person@example.com");
        assert_eq!(profile.name.as_deref(), Some("A Person"));
        assert_eq!(
            profile.image_url.as_deref(),
            Some("https://example.com/a.png")
        );
    }

    /// The rule that stands from the previous design: an unverified address
    /// links to nobody, because the store falls back to resolving by normalized
    /// email and would otherwise let one provider account adopt another
    /// person's records.
    #[test]
    fn github_without_a_verified_primary_address_yields_nobody() {
        for emails in [
            &br#"[{"email":"person@example.com","primary":true,"verified":false}]"#[..],
            &br#"[{"email":"person@example.com","primary":false,"verified":true}]"#[..],
            &b"[]"[..],
        ] {
            let error = github_profile(br#"{"id":42}"#, emails).expect_err("no person");
            assert!(matches!(error, HandshakeError::Unusable(_)), "{error}");
        }
    }

    #[test]
    fn a_google_person_is_built_from_a_checked_id_token() {
        let now = OffsetDateTime::now_utc();
        let profile =
            google_profile(&id_token(&google_fixture(now)), "goog-client", now).expect("a person");
        assert_eq!(profile.provider, Provider::Google);
        assert_eq!(profile.provider_account_id.as_str(), "1234567890");
        assert_eq!(profile.email.as_str(), "person@example.com");
    }

    #[test]
    fn an_id_token_for_another_client_is_refused() {
        let now = OffsetDateTime::now_utc();
        let error = google_profile(&id_token(&google_fixture(now)), "somebody-else", now)
            .expect_err("no person");
        assert!(matches!(error, HandshakeError::Unusable(_)), "{error}");
    }

    #[test]
    fn every_claim_this_platform_acts_on_is_checked() {
        let now = OffsetDateTime::now_utc();
        for (field, value) in [
            ("iss", serde_json::json!("https://accounts.evil.example")),
            ("aud", serde_json::json!("another-client")),
            ("exp", serde_json::json!(now.unix_timestamp() - 1)),
            ("iat", serde_json::json!(now.unix_timestamp() + 3_600)),
            ("email_verified", serde_json::json!(false)),
            ("email", serde_json::Value::Null),
        ] {
            let mut claims = google_fixture(now);
            claims[field] = value;
            let error = google_profile(&id_token(&claims), "goog-client", now)
                .unwrap_err_or_else_message(field);
            assert!(matches!(error, HandshakeError::Unusable(_)), "{field}");
        }
    }

    #[test]
    fn a_token_that_is_not_a_jwt_is_refused_before_any_claim_is_read() {
        let now = OffsetDateTime::now_utc();
        for forged in ["", "one.two", "one.two.three.four", "one.!!!.three"] {
            let error = google_profile(forged, "goog-client", now).expect_err(forged);
            assert!(matches!(error, HandshakeError::Unusable(_)), "{forged}");
        }
    }

    /// A provider that answers with a script URL must not have it written into
    /// a field the dashboard renders.
    #[test]
    fn a_non_https_avatar_is_dropped_rather_than_stored() {
        let profile = github_profile(
            br#"{"id":42,"avatar_url":"javascript:alert(1)"}"#,
            br#"[{"email":"person@example.com","primary":true,"verified":true}]"#,
        )
        .expect("a person");
        assert_eq!(profile.image_url, None);
    }

    /// `user_name_len_ck` is `char_length(name) BETWEEN 1 AND 128`. A longer one
    /// is dropped rather than refused, so a person is never locked out by the
    /// length of their own display name.
    #[test]
    fn a_name_the_database_would_refuse_is_dropped_rather_than_failing_the_sign_in() {
        let long = "x".repeat(129);
        let profile = github_profile(
            format!(r#"{{"id":42,"name":"{long}"}}"#).as_bytes(),
            br#"[{"email":"person@example.com","primary":true,"verified":true}]"#,
        )
        .expect("a person");
        assert_eq!(profile.name, None);
    }

    #[test]
    fn a_diagnostic_never_carries_the_client_secret_or_a_credential_shaped_run() {
        let rendered = redact(
            "error connecting with client_secret=gh-secret and token gho_0123456789abcdefghijklmnop",
            "gh-secret",
        );
        assert!(!rendered.contains("gh-secret"), "{rendered}");
        assert!(
            !rendered.contains("gho_0123456789abcdefghijklmnop"),
            "{rendered}"
        );
    }

    #[test]
    fn the_github_api_version_is_pinned() {
        assert_eq!(GITHUB_API_VERSION, "2026-03-10");
    }

    #[test]
    fn the_pinned_client_policy_builds() {
        HttpProviderHandshake::new(
            clients(),
            "https://dash.aex.dev/auth/callback".to_owned(),
            Duration::from_secs(5),
        )
        .expect("the pinned configuration builds");
    }

    /// Naming the field in the panic message; `expect_err` cannot.
    trait ExpectErrNamed<T, E> {
        fn unwrap_err_or_else_message(self, field: &str) -> E;
    }

    impl<T, E> ExpectErrNamed<T, E> for Result<T, E> {
        fn unwrap_err_or_else_message(self, field: &str) -> E {
            match self {
                Ok(_) => panic!("`{field}` was accepted"),
                Err(error) => error,
            }
        }
    }
}
