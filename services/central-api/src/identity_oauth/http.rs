//! Hardened transport and standard OAuth provider adapters.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use aex_identity_app::use_cases::OauthProfile;
use aex_identity_domain::Provider;
use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::{
    AuthType, AuthorizationCode, Client, ClientId, ClientSecret, EndpointNotSet, EndpointSet,
    ErrorResponse, ExtraTokenFields, HttpRequest, HttpResponse, PkceCodeVerifier, RedirectUrl,
    RequestTokenError, StandardRevocableToken, StandardTokenResponse, TokenResponse as _, TokenUrl,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::client::OauthClient;
use super::profile::{GitHubEmail, GitHubUser, github_profile, google_profile};
use super::{HandshakeError, ProviderHandshake, challenge_of};

const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GITHUB_TOKEN_ENDPOINT: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_ENDPOINT: &str = "https://api.github.com/user";
const GITHUB_EMAILS_ENDPOINT: &str = "https://api.github.com/user/emails";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Compiled provider endpoints. No production caller can redirect a credential
/// to another authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Endpoints {
    pub(super) google_token: String,
    pub(super) github_token: String,
    pub(super) github_user: String,
    pub(super) github_emails: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            google_token: GOOGLE_TOKEN_ENDPOINT.to_owned(),
            github_token: GITHUB_TOKEN_ENDPOINT.to_owned(),
            github_user: GITHUB_USER_ENDPOINT.to_owned(),
            github_emails: GITHUB_EMAILS_ENDPOINT.to_owned(),
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
    github: OauthClient,
    redirect_uri: RedirectUrl,
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
        github: OauthClient,
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
        let redirect_uri = RedirectUrl::new(redirect_uri)
            .map_err(|_| ClientBuildError("the sign-in redirect URI is invalid".to_owned()))?;
        let endpoints = Endpoints::default();
        for (name, endpoint) in [
            ("Google", &endpoints.google_token),
            ("GitHub", &endpoints.github_token),
        ] {
            TokenUrl::new(endpoint.clone())
                .map_err(|_| ClientBuildError(format!("{name}'s compiled token URI is invalid")))?;
        }
        Ok(Self {
            http: HardenedHttp { client: http },
            google,
            github,
            redirect_uri,
            endpoints,
            deadline,
        })
    }

    fn client<TR>(
        &self,
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
            })?)
            .set_redirect_uri(self.redirect_uri.clone()))
    }

    async fn google(&self, code: &str, verifier: &str) -> Result<OauthProfile, HandshakeError> {
        let response: GoogleTokenResponse = self
            .client::<GoogleTokenResponse>(&self.google, &self.endpoints.google_token)?
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

    async fn github(&self, code: &str, verifier: &str) -> Result<OauthProfile, HandshakeError> {
        let response = self
            .client::<oauth2::basic::BasicTokenResponse>(
                &self.github,
                &self.endpoints.github_token,
            )?
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
            .request_async(&self.http)
            .await
            .map_err(|error| token_error("GitHub", error, &self.github.secret))?;
        let token = response.access_token().secret();
        let user = self
            .github_get::<GitHubUser>(&self.endpoints.github_user, token)
            .await?;
        let emails = self
            .github_get::<Vec<GitHubEmail>>(&self.endpoints.github_emails, token)
            .await?;
        github_profile(user, emails)
    }

    async fn github_get<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        token: &str,
    ) -> Result<T, HandshakeError> {
        let response = self
            .http
            .client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| HandshakeError::Unreachable(redact(&error.to_string(), token)))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(HandshakeError::RateLimited);
        }
        let body = bounded_body(response, token).await.map_err(http_error)?;
        if status.is_client_error() {
            return Err(HandshakeError::Refused(format!(
                "GitHub refused the authenticated identity request with {status}"
            )));
        }
        if !status.is_success() {
            return Err(HandshakeError::Unreachable(format!(
                "GitHub's identity endpoint answered {status}"
            )));
        }
        serde_json::from_slice(&body).map_err(|_| {
            HandshakeError::Unusable("GitHub returned an undocumented identity response".to_owned())
        })
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
        let exchange = async {
            match provider {
                Provider::GitHub => self.github(code, verifier).await,
                Provider::Google => self.google(code, verifier).await,
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

fn http_error(error: ProviderHttpError) -> HandshakeError {
    match error {
        ProviderHttpError::RateLimited => HandshakeError::RateLimited,
        ProviderHttpError::TooLarge => HandshakeError::Unusable(format!(
            "the provider answered with more than {MAX_RESPONSE_BYTES} bytes"
        )),
        other => HandshakeError::Unreachable(other.to_string()),
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
