//! Hardened transport and standard OAuth provider adapters.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::{
    AuthType, AuthorizationCode, Client, ClientId, ClientSecret, EndpointNotSet, EndpointSet,
    ErrorResponse, ExtraTokenFields, HttpRequest, HttpResponse, PkceCodeVerifier, RedirectUrl,
    RequestTokenError, StandardRevocableToken, StandardTokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aex_wire::models::AuthClient;

use super::client::OauthClient;
use super::profile::google_profile;
use super::{CLI_REDIRECT_URI, HandshakeError, ProviderHandshake, challenge_of};

const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Compiled provider endpoints. No production caller can redirect a credential
/// to another authority.
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

#[derive(Clone)]
struct HardenedHttp {
    client: reqwest::Client,
}

impl std::fmt::Debug for HardenedHttp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HardenedHttp")
    }
}

#[derive(Debug, thiserror::Error)]
enum ProviderHttpError {
    #[error("the provider rate-limited the request")]
    RateLimited,
    #[error("the provider response exceeded its byte ceiling")]
    TooLarge,
    #[error("the provider request failed: {0}")]
    Unreachable(String),
    #[error("the provider response could not be represented")]
    InvalidResponse,
}

impl<'client> oauth2::AsyncHttpClient<'client> for HardenedHttp {
    type Error = ProviderHttpError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Self::Error>> + Send + 'client>>;

    fn call(&'client self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            // `oauth2` owns credential encoding. This executor never formats
            // the request, body or headers into a diagnostic.
            let response = self
                .client
                .request(request.method().clone(), request.uri().to_string())
                .headers(request.headers().clone())
                .body(request.into_body())
                .send()
                .await
                .map_err(|error| ProviderHttpError::Unreachable(redact(&error.to_string(), "")))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(ProviderHttpError::RateLimited);
            }
            let status = response.status();
            let headers = response.headers().clone();
            let body = bounded_body(response, "").await?;
            let mut answer = oauth2::http::Response::builder().status(status);
            *answer
                .headers_mut()
                .ok_or(ProviderHttpError::InvalidResponse)? = headers;
            answer
                .body(body)
                .map_err(|_| ProviderHttpError::InvalidResponse)
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GoogleTokenFields {
    id_token: Option<String>,
}

impl ExtraTokenFields for GoogleTokenFields {}

type GoogleTokenResponse = StandardTokenResponse<GoogleTokenFields, BasicTokenType>;
type ProviderClient<TR> = Client<
    BasicErrorResponse,
    TR,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

/// The handshake over pinned HTTPS provider origins.
#[derive(Debug)]
pub struct HttpProviderHandshake {
    http: HardenedHttp,
    google: OauthClient,
    dashboard_redirect_uri: RedirectUrl,
    cli_redirect_uri: RedirectUrl,
    endpoints: Endpoints,
    deadline: Duration,
}

/// Why the provider clients could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("provider client configuration was refused: {0}")]
pub struct ClientBuildError(String);

impl HttpProviderHandshake {
    /// Builds the handshake against the compiled provider origins.
    ///
    /// # Errors
    ///
    /// Returns [`ClientBuildError`] when the hardened executor or a configured
    /// URI is invalid.
    pub fn new(
        google: OauthClient,
        redirect_uri: String,
        deadline: Duration,
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
            .map_err(|_| ClientBuildError("the HTTPS executor could not be built".to_owned()))?;
        let dashboard_redirect_uri = RedirectUrl::new(redirect_uri)
            .map_err(|_| ClientBuildError("the sign-in redirect URI is invalid".to_owned()))?;
        let cli_redirect_uri = RedirectUrl::new(CLI_REDIRECT_URI.to_owned())
            .map_err(|_| ClientBuildError("the compiled CLI redirect URI is invalid".to_owned()))?;
        let endpoints = Endpoints::default();
        TokenUrl::new(endpoints.google_token.clone())
            .map_err(|_| ClientBuildError("Google's compiled token URI is invalid".to_owned()))?;
        Ok(Self {
            http: HardenedHttp { client: http },
            google,
            dashboard_redirect_uri,
            cli_redirect_uri,
            endpoints,
            deadline,
        })
    }

    fn client<TR>(
        configured: &OauthClient,
        token_endpoint: &str,
    ) -> Result<ProviderClient<TR>, HandshakeError>
    where
        TR: oauth2::TokenResponse,
    {
        Ok(Client::new(ClientId::new(configured.id().to_owned()))
            .set_client_secret(ClientSecret::new(configured.secret.clone()))
            .set_auth_type(AuthType::RequestBody)
            .set_token_uri(TokenUrl::new(token_endpoint.to_owned()).map_err(|_| {
                HandshakeError::Unusable("a compiled provider token URI is invalid".to_owned())
            })?))
    }

    pub(super) fn redirect_uri(&self, client: AuthClient) -> &RedirectUrl {
        match client {
            AuthClient::Dashboard => &self.dashboard_redirect_uri,
            AuthClient::Cli => &self.cli_redirect_uri,
        }
    }

    async fn google(
        &self,
        client: AuthClient,
        code: &str,
        verifier: &str,
    ) -> Result<OauthProfile, HandshakeError> {
        let response: GoogleTokenResponse =
            Self::client::<GoogleTokenResponse>(&self.google, &self.endpoints.google_token)?
                .set_redirect_uri(self.redirect_uri(client).clone())
                .exchange_code(AuthorizationCode::new(code.to_owned()))
                .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
                .request_async(&self.http)
                .await
                .map_err(|error| token_error("Google", error, &self.google.secret))?;
        let id_token = response
            .extra_fields()
            .id_token
            .as_deref()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                HandshakeError::Unusable("Google answered without an ID token".to_owned())
            })?;
        google_profile(
            id_token,
            self.google.id(),
            &challenge_of(verifier),
            OffsetDateTime::now_utc(),
        )
    }
}

#[async_trait::async_trait]
impl ProviderHandshake for HttpProviderHandshake {
    fn public_client_id(&self) -> &str {
        self.google.id()
    }

    async fn identify(
        &self,
        client: AuthClient,
        code: &str,
        verifier: &str,
    ) -> Result<OauthProfile, HandshakeError> {
        let exchange = self.google(client, code, verifier);
        tokio::time::timeout(self.deadline, exchange)
            .await
            .map_err(|_| {
                HandshakeError::Unreachable(
                    "the provider did not answer inside this request's deadline".to_owned(),
                )
            })?
    }
}

fn token_error<T>(
    provider: &str,
    error: RequestTokenError<ProviderHttpError, T>,
    secret: &str,
) -> HandshakeError
where
    T: ErrorResponse + 'static,
{
    match error {
        RequestTokenError::ServerResponse(detail) => {
            HandshakeError::Refused(redact(&format!("{provider}: {detail}"), secret))
        }
        RequestTokenError::Request(ProviderHttpError::RateLimited) => HandshakeError::RateLimited,
        RequestTokenError::Request(other) => HandshakeError::Unreachable(other.to_string()),
        RequestTokenError::Parse(_, _) => {
            HandshakeError::Unreachable(format!("{provider} returned an invalid token response"))
        }
        RequestTokenError::Other(detail) => HandshakeError::Unusable(redact(&detail, secret)),
    }
}

async fn bounded_body(
    mut response: reqwest::Response,
    secret: &str,
) -> Result<Vec<u8>, ProviderHttpError> {
    let mut buffer = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|error| ProviderHttpError::Unreachable(redact(&error.to_string(), secret)))?;
        let Some(chunk) = chunk else { break };
        if buffer.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ProviderHttpError::TooLarge);
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

/// Replaces a known secret and every long credential-shaped run in a bounded
/// diagnostic. No request body, authorization header or token is ever formatted
/// before reaching this function.
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
