//! Native first-login account creation over Google OAuth and the fixed loopback callback.

use std::sync::Arc;
use std::time::Duration;

use aex_wire::client::{BaseUrl, WireClient};
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{AccountId, UserId};
use aex_wire::models::{
    AccountOperationalState, ApiKeyCreateRequest, AuthClient, CliAuthConfig,
    DashboardSessionRequest, NewApiKey, Workspace,
};
use base64::Engine as _;
use rand::TryRng as _;
use serde::Serialize;
use sha2::Digest as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use zeroize::{Zeroize as _, Zeroizing};

use crate::output::OutputFormat;
use crate::runtime::{HttpTransport, RuntimeError, emit, workspace_key_scopes};

const CLI_CALLBACK_ADDR: &str = "127.0.0.1:53682";
const CLI_CALLBACK_PATH: &str = "/callback";
const CLI_REDIRECT_URI: &str = "http://127.0.0.1:53682/callback";
const GOOGLE_AUTHORIZATION_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const CLI_CALLBACK_TIMEOUT: Duration = Duration::from_mins(5);
const CALLBACK_REQUEST_MAX: usize = 8 * 1024;
const AUTHORIZATION_CODE_MAX: usize = 2_048;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountCreateOutput<'a> {
    account_id: &'a AccountId,
    account_state: &'a AccountOperationalState,
    email: &'a str,
    user_id: &'a UserId,
    workspace: &'a Workspace,
    api_key: &'a NewApiKey,
}

struct PkceAttempt {
    verifier: Zeroizing<String>,
    challenge: String,
}

/// Performs one native first-login ceremony and returns the new workspace key once.
pub(crate) async fn create(
    api_key_name: &str,
    central_base: BaseUrl,
    key: &IdempotencyKey,
    output: OutputFormat,
    quiet: bool,
) -> Result<(), RuntimeError> {
    let anonymous_transport = HttpTransport::anonymous(central_base.clone())?;
    let anonymous = WireClient::new(anonymous_transport, central_base.clone());
    let config = anonymous.auth_config_get().await?;
    validate_cli_auth_config(&config)?;

    let listener = TcpListener::bind(CLI_CALLBACK_ADDR).await.map_err(|_| {
        RuntimeError::local_io("the fixed CLI sign-in callback port 127.0.0.1:53682 is unavailable")
    })?;
    let pkce = new_pkce_attempt()?;
    let authorization_url = google_authorization_url(&config, &pkce.challenge)?;
    webbrowser::open(authorization_url.as_str())
        .map_err(|_| RuntimeError::local_io("the system browser could not be opened"))?;

    let code = await_callback(listener, &pkce.challenge).await?;
    let mut exchange = DashboardSessionRequest {
        client: AuthClient::Cli,
        code,
        code_verifier: pkce.verifier.as_str().to_owned(),
        state: pkce.challenge,
    };
    let session_result = anonymous.dashboard_session_create(&exchange).await;
    exchange.code.zeroize();
    exchange.code_verifier.zeroize();
    let session = session_result?;

    let session_secret = Arc::new(Zeroizing::new(session.session));
    let session_transport = HttpTransport::new(central_base.clone(), session_secret)?;
    let authenticated = WireClient::new(session_transport, central_base);
    let provisioned = async {
        let bootstrap = authenticated.dashboard_bootstrap_get().await?;
        let created = authenticated
            .api_key_create(
                &ApiKeyCreateRequest {
                    name: api_key_name.to_owned(),
                    scopes: workspace_key_scopes(&[])?,
                    workspace_id: bootstrap.workspace.id,
                },
                key,
            )
            .await?;
        Ok::<_, RuntimeError>((bootstrap, created))
    }
    .await;
    if authenticated.dashboard_session_delete().await.is_err() && !quiet {
        eprintln!(
            "warning: the temporary dashboard session could not be closed; it will expire automatically"
        );
    }

    let (bootstrap, mut created) = provisioned?;
    let emitted = emit(
        &AccountCreateOutput {
            account_id: &bootstrap.account_id,
            account_state: &bootstrap.account_state,
            email: &bootstrap.email,
            user_id: &bootstrap.user_id,
            workspace: &bootstrap.workspace,
            api_key: &created,
        },
        output,
    );
    created.value.zeroize();
    emitted
}

fn validate_cli_auth_config(config: &CliAuthConfig) -> Result<(), RuntimeError> {
    if config.authorization_url.as_str() != GOOGLE_AUTHORIZATION_URL
        || config.redirect_uri != CLI_REDIRECT_URI
    {
        return Err(RuntimeError::configuration(
            "the central API returned an unsupported CLI sign-in configuration",
        ));
    }
    Ok(())
}

fn new_pkce_attempt() -> Result<PkceAttempt, RuntimeError> {
    let mut entropy = [0_u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut entropy)
        .map_err(|_| RuntimeError::unavailable("secure randomness is unavailable"))?;
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(entropy);
    entropy.zeroize();
    let challenge = pkce_challenge(&verifier);
    Ok(PkceAttempt {
        verifier: Zeroizing::new(verifier),
        challenge,
    })
}

fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

fn google_authorization_url(
    config: &CliAuthConfig,
    challenge: &str,
) -> Result<url::Url, RuntimeError> {
    validate_cli_auth_config(config)?;
    let mut url = url::Url::parse(config.authorization_url.as_str())
        .map_err(|_| RuntimeError::configuration("the CLI sign-in authorization URL is invalid"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", challenge)
        .append_pair("nonce", challenge);
    Ok(url)
}

async fn await_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, RuntimeError> {
    tokio::time::timeout(
        CLI_CALLBACK_TIMEOUT,
        receive_callback(listener, expected_state),
    )
    .await
    .map_err(|_| RuntimeError::unavailable("Google sign-in timed out"))?
}

async fn receive_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, RuntimeError> {
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|_| RuntimeError::local_io("the CLI sign-in callback could not be accepted"))?;
    let request = read_callback_request(&mut stream).await?;
    let parsed = parse_callback_request(&request, expected_state);
    write_callback_response(&mut stream, parsed.is_ok()).await;
    parsed
}

async fn read_callback_request(stream: &mut TcpStream) -> Result<Vec<u8>, RuntimeError> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| RuntimeError::authentication("the CLI sign-in callback is invalid"))?;
        if read == 0 {
            return Err(RuntimeError::authentication(
                "the CLI sign-in callback ended before its headers",
            ));
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
        if request.len() > CALLBACK_REQUEST_MAX {
            return Err(RuntimeError::authentication(
                "the CLI sign-in callback is too large",
            ));
        }
    }
}

fn parse_callback_request(request: &[u8], expected_state: &str) -> Result<String, RuntimeError> {
    if request.len() > CALLBACK_REQUEST_MAX {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback is too large",
        ));
    }
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            RuntimeError::authentication("the CLI sign-in callback has incomplete headers")
        })?;
    let headers = std::str::from_utf8(&request[..header_end])
        .map_err(|_| RuntimeError::authentication("the CLI sign-in callback is not UTF-8"))?;
    let request_line = headers.lines().next().ok_or_else(|| {
        RuntimeError::authentication("the CLI sign-in callback has no request line")
    })?;
    let mut parts = request_line.split(' ');
    let (Some("GET"), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback request line is invalid",
        ));
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !target.starts_with('/') {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback request line is invalid",
        ));
    }
    let mut host = None;
    for header in headers.lines().skip(1) {
        let Some((name, value)) = header.split_once(':') else {
            return Err(RuntimeError::authentication(
                "the CLI sign-in callback has an invalid header",
            ));
        };
        if name.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            return Err(RuntimeError::authentication(
                "the CLI sign-in callback repeats its host",
            ));
        }
    }
    if host != Some(CLI_CALLBACK_ADDR) {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback host is invalid",
        ));
    }
    let callback = url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| RuntimeError::authentication("the CLI sign-in callback URL is invalid"))?;
    if callback.path() != CLI_CALLBACK_PATH || callback.fragment().is_some() {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback path is invalid",
        ));
    }

    let mut code = None;
    let mut state = None;
    let mut provider_error = false;
    for (name, value) in callback.query_pairs() {
        match name.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "code" => {
                return Err(RuntimeError::authentication(
                    "the CLI sign-in callback repeats its authorization code",
                ));
            }
            "state" if state.is_none() => state = Some(value.into_owned()),
            "state" => {
                return Err(RuntimeError::authentication(
                    "the CLI sign-in callback repeats its state",
                ));
            }
            "error" => provider_error = true,
            _ => {}
        }
    }
    let state = state
        .ok_or_else(|| RuntimeError::authentication("the CLI sign-in callback has no state"))?;
    if state != expected_state {
        return Err(RuntimeError::authentication(
            "the CLI sign-in callback state does not match",
        ));
    }
    if provider_error {
        return Err(RuntimeError::authentication(
            "Google sign-in was not completed",
        ));
    }
    let code = code.ok_or_else(|| {
        RuntimeError::authentication("the CLI sign-in callback has no authorization code")
    })?;
    if code.is_empty() || code.len() > AUTHORIZATION_CODE_MAX || code.chars().any(char::is_control)
    {
        return Err(RuntimeError::authentication(
            "the CLI sign-in authorization code is invalid",
        ));
    }
    Ok(code)
}

async fn write_callback_response(stream: &mut TcpStream, success: bool) {
    let (status, body) = if success {
        ("200 OK", "AEX sign-in complete. You can close this tab.")
    } else {
        ("400 Bad Request", "AEX sign-in failed. Return to the CLI.")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aex_wire::types::HttpsUrl;

    use super::*;

    fn cli_auth_config() -> CliAuthConfig {
        CliAuthConfig {
            authorization_url: HttpsUrl::parse(GOOGLE_AUTHORIZATION_URL).expect("pinned URL"),
            client_id: "google-public-client".to_owned(),
            redirect_uri: CLI_REDIRECT_URI.to_owned(),
        }
    }

    #[test]
    fn pkce_uses_the_rfc_7636_s256_derivation_and_secure_length() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let generated = new_pkce_attempt().expect("operating-system randomness");
        assert_eq!(generated.verifier.len(), 43);
        assert_eq!(generated.challenge, pkce_challenge(&generated.verifier));
    }

    #[test]
    fn authorization_url_is_fully_pinned_and_never_contains_the_verifier() {
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let url = google_authorization_url(&cli_auth_config(), challenge).expect("authorization");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        assert_eq!(url.path(), "/o/oauth2/v2/auth");
        let query = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(query["client_id"], "google-public-client");
        assert_eq!(query["redirect_uri"], CLI_REDIRECT_URI);
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["scope"], "openid email profile");
        assert_eq!(query["code_challenge"], challenge);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["state"], challenge);
        assert_eq!(query["nonce"], challenge);
        assert!(!url.as_str().contains("dBjftJeZ4CVP"));

        let mut wrong_endpoint = cli_auth_config();
        wrong_endpoint.authorization_url =
            HttpsUrl::parse("https://example.com/authorize").expect("HTTPS");
        assert!(google_authorization_url(&wrong_endpoint, challenge).is_err());
        let mut wrong_callback = cli_auth_config();
        wrong_callback.redirect_uri = "http://127.0.0.1:53682/other".to_owned();
        assert!(google_authorization_url(&wrong_callback, challenge).is_err());
    }

    #[test]
    fn callback_parser_requires_exact_path_state_single_code_and_bounds() {
        let state = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let valid = format!(
            "GET /callback?code=google-code&state={state}&scope=openid HTTP/1.1\r\nHost: 127.0.0.1:53682\r\n\r\n"
        );
        assert_eq!(
            parse_callback_request(valid.as_bytes(), state).expect("callback"),
            "google-code"
        );
        for invalid in [
            format!(
                "GET /callback?code=google-code&state={state} HTTP/1.1\r\nHost: localhost:53682\r\n\r\n"
            ),
            format!(
                "GET /wrong?code=google-code&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            ),
            "GET /callback?code=google-code&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n"
                .to_owned(),
            format!(
                "GET /callback?code=one&code=two&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            ),
            format!(
                "GET /callback?error=access_denied&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            ),
            format!(
                "GET /callback?code={}&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                "x".repeat(AUTHORIZATION_CODE_MAX + 1)
            ),
        ] {
            assert!(
                parse_callback_request(invalid.as_bytes(), state).is_err(),
                "invalid callback was accepted"
            );
        }
    }
}
