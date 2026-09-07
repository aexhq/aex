use crate::{
    config::Config,
    error::{Error, Result},
};
use axum::http::{HeaderMap, Method};
use bytes::Bytes;
use futures_util::StreamExt;
use std::time::Duration;

#[derive(Clone)]
pub struct Brain {
    client: reqwest::Client,
    origin: String,
    token: String,
    timeout: Duration,
    response_bytes: usize,
}
impl Brain {
    pub fn new(config: &Config, token: String) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(config.limits.requests)
            .build()?;
        Ok(Self {
            client,
            origin: config.brain_url.trim_end_matches('/').into(),
            token,
            timeout: Duration::from_secs(config.limits.upstream_timeout_secs),
            response_bytes: config.limits.response_bytes,
        })
    }
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        key: Option<&str>,
        host_token: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(host_token.unwrap_or(&self.token));
        for name in ["content-type", "accept"] {
            if let Some(value) = headers.get(name) {
                request = request.header(name, value);
            }
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        tokio::time::timeout(self.timeout, request.body(body).send())
            .await
            .map_err(|_| Error::ambiguous())?
            .map_err(|_| Error::ambiguous())
    }
    pub async fn bytes(&self, response: reqwest::Response) -> Result<Bytes> {
        let limit = self.response_bytes;
        tokio::time::timeout(self.timeout, async move {
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| Error::ambiguous())?;
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(Error::capacity());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes.into())
        })
        .await
        .map_err(|_| Error::ambiguous())?
    }
    pub async fn summary(&self, id: &str) -> Result<brain_protocol::SessionSummary> {
        let response = self
            .request(
                Method::GET,
                &format!("/v1/sessions/{id}"),
                &HeaderMap::new(),
                Bytes::new(),
                None,
                None,
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::ambiguous());
        }
        serde_json::from_slice(&self.bytes(response).await?).map_err(|_| Error::internal())
    }
    pub async fn ready(&self) -> bool {
        self.request(
            Method::GET,
            "/health/ready",
            &HeaderMap::new(),
            Bytes::new(),
            None,
            None,
        )
        .await
        .is_ok_and(|r| r.status().is_success())
    }
}
