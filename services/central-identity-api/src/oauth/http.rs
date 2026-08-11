//! Pinned HTTPS transport for provider OAuth handshakes.

use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::Provider;
use time::OffsetDateTime;

use super::client::OauthClient;
use super::profile::{google_profile, parse_google_token};
use super::{HandshakeError, ProviderHandshake};

const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Where each provider is reached.
///
/// Compiled constants in production. The overriding constructor exists only for
/// this crate's own tests, which keeps caller input from choosing who receives a
/// client secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Endpoints {
    pub(super) google_token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            google_token: GOOGLE_TOKEN_ENDPOINT.to_owned(),
        }
    }
}

/// The handshake over HTTPS.
#[derive(Debug)]
pub struct HttpProviderHandshake {
    http: reqwest::Client,
    client: OauthClient,
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
    /// `deadline` bounds the whole handshake rather than each call inside it.
    ///
    /// # Errors
    ///
    /// Returns [`ClientBuildError`] when `reqwest` refuses the pinned policy.
    pub fn new(
        client: OauthClient,
        redirect_uri: String,
        deadline: Duration,
    ) -> Result<Self, ClientBuildError> {
        Self::with_endpoints(client, redirect_uri, deadline, Endpoints::default())
    }

    pub(super) fn with_endpoints(
        client: OauthClient,
        redirect_uri: String,
        deadline: Duration,
        endpoints: Endpoints,
    ) -> Result<Self, ClientBuildError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .use_rustls_tls()
            .https_only(true)
            .user_agent(concat!(
                "aex-central-identity-api/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|_| ClientBuildError)?;
        Ok(Self {
            http,
            client,
            redirect_uri,
            endpoints,
            deadline,
        })
    }

    /// The single site that hands a request to the network.
    ///
    /// It is called at most once per provider round trip and never retried: an
    /// authorization code that may already be spent cannot be presented again.
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

    async fn google(
        &self,
        code: &str,
        verifier: &str,
        now: OffsetDateTime,
    ) -> Result<OauthProfile, HandshakeError> {
        let client = &self.client;
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
pub(super) fn form(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

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
pub(super) fn redact(text: &str, secret: &str) -> String {
    let bounded: String = text.chars().take(256).collect();
    let replaced = if secret.is_empty() {
        bounded
    } else {
        bounded.replace(secret, "[redacted]")
    };
    mask_runs(&replaced)
}

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
