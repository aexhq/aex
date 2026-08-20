//! The Aex client SDK — a typed client over the generated contracts.
//!
//! Responses deserialize into `aex_contracts::control` and Brain-owned
//! `brain_protocol::session` types (the same JSON Schemas the server is pinned to), so a wire
//! drift is a decode error here, not a
//! silent surprise. Request bodies are anything serializable: the server validates strictly.
//!
//! Two credentials, two jobs, same as the API: the account token (`aex_at_`) for identity and
//! billing, an API key (`aex_sk_`) for session work.

use aex_contracts::control;
use brain_protocol::session;
use futures_util::StreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// The server answered with an error envelope.
    #[error("{status} {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("{0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, SdkError>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    account_token: Option<String>,
    api_key: Option<String>,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Client {
            http: reqwest::Client::new(),
            base,
            account_token: None,
            api_key: None,
        }
    }

    pub fn with_account_token(mut self, token: impl Into<String>) -> Self {
        self.account_token = Some(token.into());
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    fn account_token(&self) -> Result<&str> {
        self.account_token
            .as_deref()
            .ok_or_else(|| SdkError::Config("no account token (aex_at_...) configured".into()))
    }

    fn api_key(&self) -> Result<&str> {
        self.api_key
            .as_deref()
            .ok_or_else(|| SdkError::Config("no API key (aex_sk_...) configured".into()))
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let resp = self.send(method, path, bearer, body).await?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| SdkError::Transport(format!("{e}")))?;
        if !(200..300).contains(&status) {
            return Err(api_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| SdkError::Decode(format!("{path}: {e}\n{text}")))
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> Result<reqwest::Response> {
        let mut req = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send()
            .await
            .map_err(|e| SdkError::Transport(format!("{e}")))
    }

    // ---- identity and billing (account token) ----

    /// Sign up. No auth; the returned account token appears once and never again.
    pub async fn signup(&self, email: &str) -> Result<control::AccountCreated> {
        self.request(
            reqwest::Method::POST,
            "/v1/accounts",
            None,
            Some(&serde_json::json!({"email": email})),
        )
        .await
    }

    pub async fn account(&self) -> Result<control::Account> {
        self.request(
            reqwest::Method::GET,
            "/v1/account",
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    /// Create an API key; the secret in the response appears once.
    pub async fn create_key(&self, name: &str) -> Result<control::ApiKeyCreated> {
        self.request(
            reqwest::Method::POST,
            "/v1/keys",
            Some(self.account_token()?),
            Some(&serde_json::json!({"name": name})),
        )
        .await
    }

    pub async fn list_keys(&self) -> Result<control::ApiKeyList> {
        self.request(
            reqwest::Method::GET,
            "/v1/keys",
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    pub async fn revoke_key(&self, key_id: &str) -> Result<()> {
        let resp = self
            .send(
                reqwest::Method::DELETE,
                &format!("/v1/keys/{key_id}"),
                Some(self.account_token()?),
                None::<&()>,
            )
            .await?;
        expect_no_content(resp).await
    }

    pub async fn balance(&self) -> Result<control::Balance> {
        self.request(
            reqwest::Method::GET,
            "/v1/balance",
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    /// Start a top-up; the customer pays at `checkout_url`. Minimum 1000 cents ($10).
    pub async fn create_topup(&self, amount_cents: i64) -> Result<control::Topup> {
        self.request(
            reqwest::Method::POST,
            "/v1/topups",
            Some(self.account_token()?),
            Some(&serde_json::json!({"amount_cents": amount_cents})),
        )
        .await
    }

    /// Poll a top-up; the server settles it idempotently when the provider reports it paid.
    pub async fn get_topup(&self, topup_id: &str) -> Result<control::Topup> {
        self.request(
            reqwest::Method::GET,
            &format!("/v1/topups/{topup_id}"),
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    pub async fn list_topups(&self) -> Result<control::TopupList> {
        self.request(
            reqwest::Method::GET,
            "/v1/topups",
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    /// The bill.
    pub async fn usage(&self) -> Result<control::Usage> {
        self.request(
            reqwest::Method::GET,
            "/v1/usage",
            Some(self.account_token()?),
            None::<&()>,
        )
        .await
    }

    /// The public rate card. No auth.
    pub async fn rates(&self) -> Result<control::RateCard> {
        self.request(reqwest::Method::GET, "/v1/rates", None, None::<&()>)
            .await
    }

    // ---- sessions (API key) ----

    /// Create a session. The body is a `session/v1 CreateSessionRequest`; build it as JSON —
    /// the server validates strictly, and BYOK means the provider key rides inside it.
    pub async fn create_session(&self, body: &impl Serialize) -> Result<session::Session> {
        self.request(
            reqwest::Method::POST,
            "/v1/sessions",
            Some(self.api_key()?),
            Some(body),
        )
        .await
    }

    pub async fn list_sessions(&self) -> Result<session::SessionList> {
        self.request(
            reqwest::Method::GET,
            "/v1/sessions",
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    pub async fn get_session(&self, session_id: &str) -> Result<session::Session> {
        self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{session_id}"),
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        body: &impl Serialize,
    ) -> Result<session::MessageAccepted> {
        self.request(
            reqwest::Method::POST,
            &format!("/v1/sessions/{session_id}/messages"),
            Some(self.api_key()?),
            Some(body),
        )
        .await
    }

    pub async fn cancel(&self, session_id: &str) -> Result<()> {
        let resp = self
            .send(
                reqwest::Method::POST,
                &format!("/v1/sessions/{session_id}/cancel"),
                Some(self.api_key()?),
                None::<&()>,
            )
            .await?;
        expect_success(resp).await
    }

    pub async fn end_session(&self, session_id: &str) -> Result<session::Session> {
        self.request(
            reqwest::Method::POST,
            &format!("/v1/sessions/{session_id}/end"),
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let resp = self
            .send(
                reqwest::Method::DELETE,
                &format!("/v1/sessions/{session_id}"),
                Some(self.api_key()?),
                None::<&()>,
            )
            .await?;
        expect_no_content(resp).await
    }

    /// Deterministically list a workspace subtree. Released remote sessions answer from the
    /// last durable manifest without waking compute.
    pub async fn list_files(
        &self,
        session_id: &str,
        path: &str,
        recursive: bool,
    ) -> Result<session::FileList> {
        self.request(
            reqwest::Method::GET,
            &format!(
                "/v1/sessions/{session_id}/files?path={}&recursive={recursive}",
                encode(path)
            ),
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    /// Download exact workspace bytes (bounded by the plane's advertised file limit).
    pub async fn download_file(&self, session_id: &str, path: &str) -> Result<bytes::Bytes> {
        let response = self
            .send(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}/files/{}", encode(path)),
                Some(self.api_key()?),
                None::<&()>,
            )
            .await?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(api_error(status, &String::from_utf8_lossy(&bytes)));
        }
        Ok(bytes)
    }

    /// Absolute-overwrite a workspace file; success means the brain checkpointed and
    /// committed the new durable hand state.
    pub async fn upload_file(
        &self,
        session_id: &str,
        path: &str,
        bytes: impl Into<bytes::Bytes>,
    ) -> Result<session::FileEntry> {
        let response = self
            .http
            .put(format!(
                "{}/v1/sessions/{session_id}/files/{}",
                self.base,
                encode(path)
            ))
            .bearer_auth(self.api_key()?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes.into())
            .send()
            .await
            .map_err(|error| SdkError::Transport(error.to_string()))?;
        decode_response(response, "upload file").await
    }

    pub async fn persist_artifact(
        &self,
        session_id: &str,
        name: &str,
        path: &str,
        media_type: Option<&str>,
    ) -> Result<session::Artifact> {
        let mut body = serde_json::json!({"name": name, "path": path});
        if let Some(media_type) = media_type {
            body["media_type"] = media_type.into();
        }
        self.request(
            reqwest::Method::POST,
            &format!("/v1/sessions/{session_id}/persist"),
            Some(self.api_key()?),
            Some(&body),
        )
        .await
    }

    pub async fn list_artifacts(&self, session_id: &str) -> Result<session::ArtifactList> {
        self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{session_id}/artifacts"),
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    pub async fn get_artifact(&self, session_id: &str, name: &str) -> Result<session::Artifact> {
        self.request(
            reqwest::Method::GET,
            &format!("/v1/sessions/{session_id}/artifacts/{}", encode(name)),
            Some(self.api_key()?),
            None::<&()>,
        )
        .await
    }

    /// Stream events (SSE) starting after `after`. `on_event` returns `false` to stop early
    /// (e.g. on `turn.completed`). With `follow=false` the stream ends at the journal head.
    pub async fn events(
        &self,
        session_id: &str,
        after: i64,
        follow: bool,
        mut on_event: impl FnMut(session::Event) -> bool,
    ) -> Result<()> {
        let resp = self
            .send(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}/events?after={after}&follow={follow}"),
                Some(self.api_key()?),
                None::<&()>,
            )
            .await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &text));
        }
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| SdkError::Transport(format!("{e}")))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE frames end on a blank line; data lines carry the event JSON.
            while let Some(pos) = buf.find("\n\n") {
                let frame: String = buf.drain(..pos + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let ev: session::Event = serde_json::from_str(data.trim_start())
                        .map_err(|e| SdkError::Decode(format!("event: {e}\n{data}")))?;
                    if !on_event(ev) {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

async fn decode_response<T: DeserializeOwned>(
    response: reqwest::Response,
    context: &str,
) -> Result<T> {
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|error| SdkError::Transport(error.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(api_error(status, &text));
    }
    serde_json::from_str(&text)
        .map_err(|error| SdkError::Decode(format!("{context}: {error}\n{text}")))
}

async fn expect_no_content(resp: reqwest::Response) -> Result<()> {
    let status = resp.status().as_u16();
    if status == 204 {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    Err(api_error(status, &text))
}

async fn expect_success(resp: reqwest::Response) -> Result<()> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    Err(api_error(status, &text))
}

fn api_error(status: u16, text: &str) -> SdkError {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    SdkError::Api {
        status,
        code: v["error"]["code"].as_str().unwrap_or("unknown").to_string(),
        message: v["error"]["message"]
            .as_str()
            .unwrap_or(if text.is_empty() { "(no body)" } else { text })
            .to_string(),
    }
}
