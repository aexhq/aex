use crate::{
    brain_contract::{EventList, Session},
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
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .pool_max_idle_per_host(config.limits.requests)
                .build()?,
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
        tenant: &str,
        executor_token: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut url = url::Url::parse(&self.origin).map_err(|_| Error::internal())?;
        let (path, query) = path.split_once('?').map_or((path, ""), |parts| parts);
        url.set_path(path);
        url.set_query((!query.is_empty()).then_some(query));
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(executor_token.unwrap_or(&self.token));
        if !body.is_empty() {
            request = request.header("content-type", "application/json");
        }
        if executor_token.is_none() {
            request = request.header("x-aex-tenant", tenant);
        }
        for name in ["content-type", "accept", "last-event-id", "x-aex-session"] {
            if let Some(value) = headers.get(name) {
                request = request.header(name, value);
            }
        }
        tokio::time::timeout(self.timeout, request.body(body).send())
            .await
            .map_err(|_| Error::ambiguous())?
            .map_err(|_| Error::ambiguous())
    }
    pub async fn bytes(&self, response: reqwest::Response) -> Result<Bytes> {
        tokio::time::timeout(self.timeout, async move {
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| Error::ambiguous())?;
                if bytes.len().saturating_add(chunk.len()) > self.response_bytes {
                    return Err(Error::capacity());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes.into())
        })
        .await
        .map_err(|_| Error::ambiguous())?
    }
    pub async fn summary(&self, tenant: &str, id: &str) -> Result<Session> {
        let response = self
            .request(
                Method::GET,
                &format!("/sessions/{id}"),
                &HeaderMap::new(),
                Bytes::new(),
                tenant,
                None,
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::ambiguous());
        }
        serde_json::from_slice(&self.bytes(response).await?).map_err(|_| Error::internal())
    }
    pub async fn history(&self, tenant: &str, id: &str, after: &str) -> Result<EventList> {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("after", after)
            .append_pair("limit", "1000")
            .finish();
        let response = self
            .request(
                Method::GET,
                &format!("/sessions/{id}/events?{query}"),
                &HeaderMap::new(),
                Bytes::new(),
                tenant,
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
            "",
            None,
        )
        .await
        .is_ok_and(|response| response.status().is_success())
    }
}
