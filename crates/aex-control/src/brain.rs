use std::{net::IpAddr, time::Duration};

use brain_protocol::{EventPage, Session, SessionId};

use crate::{Error, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone)]
pub struct BrainClient {
    http: reqwest::Client,
    base: reqwest::Url,
    token: String,
}

impl BrainClient {
    pub fn new(base: impl AsRef<str>, token: impl Into<String>) -> anyhow::Result<Self> {
        let mut base = reqwest::Url::parse(base.as_ref())?;
        let loopback_http = base.scheme() == "http"
            && base
                .host_str()
                .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
        anyhow::ensure!(
            base.scheme() == "https" || loopback_http,
            "AEX_BRAIN_URL must use HTTPS or literal loopback HTTP"
        );
        anyhow::ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "AEX_BRAIN_URL cannot contain credentials, query, or fragment"
        );
        let token = token.into();
        anyhow::ensure!(!token.is_empty(), "AEX_BRAIN_TOKEN cannot be empty");
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self { http, base, token })
    }

    pub async fn forward(
        &self,
        method: reqwest::Method,
        path_and_query: &str,
        content_type: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<bytes::Bytes>,
    ) -> Result<reqwest::Response> {
        if !path_and_query.starts_with("/v1/")
            && path_and_query != "/v1/sessions"
            && !path_and_query.starts_with("/health/")
        {
            return Err(Error::Invalid("invalid Brain path".into()));
        }
        let target = self
            .base
            .join(path_and_query.trim_start_matches('/'))
            .map_err(|error| Error::Internal(format!("Brain URL: {error}")))?;
        let mut request = self
            .http
            .request(method, target)
            .bearer_auth(&self.token)
            .timeout(REQUEST_TIMEOUT);
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        request
            .send()
            .await
            .map_err(|error| Error::Upstream(error.to_string()))
    }

    pub async fn session(&self, session_id: &str) -> Result<Option<Session>> {
        let id = SessionId::new(session_id.to_owned());
        let response = self
            .forward(
                reqwest::Method::GET,
                &format!("/v1/sessions/{id}"),
                None,
                None,
                None,
            )
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::Upstream(format!(
                "Brain returned {} while reading a session",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|error| Error::Upstream(format!("Brain session response: {error}")))
    }

    pub async fn events(&self, session_id: &str, after: u64) -> Result<EventPage> {
        let response = self
            .forward(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}/events?after={after}"),
                None,
                None,
                None,
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::Upstream(format!(
                "Brain returned {} while reading session events",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|error| Error::Upstream(format!("Brain event response: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_brain_requires_a_safe_origin() {
        assert!(BrainClient::new("https://brain.example", "token").is_ok());
        assert!(BrainClient::new("http://127.0.0.1:8700", "token").is_ok());
        assert!(BrainClient::new("http://brain.example", "token").is_err());
        assert!(BrainClient::new("https://secret@brain.example", "token").is_err());
    }
}
