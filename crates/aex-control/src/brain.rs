//! The brain, seen from the control plane: a session/v1 server we hold the operator token for.
//!
//! Two uses: verbatim forwarding of customer requests (the authority proxy), and reading the
//! journal for billing — `GET /events?follow=false` replay (the journal IS the SSE event log)
//! plus the session document for the byte meters.

use serde_json::Value;

use crate::{Error, Result};

#[derive(Clone)]
pub struct BrainClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl BrainClient {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        let mut base = base.into();
        while base.ends_with('/') {
            base.pop();
        }
        BrainClient {
            http: reqwest::Client::new(),
            base,
            token: token.into(),
        }
    }

    /// Forward a request as the operator. `path_and_query` starts with `/v1/...`.
    pub async fn forward(
        &self,
        method: reqwest::Method,
        path_and_query: &str,
        content_type: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<bytes::Bytes>,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .request(method, format!("{}{path_and_query}", self.base))
            .bearer_auth(&self.token);
        if let Some(ct) = content_type {
            req = req.header(reqwest::header::CONTENT_TYPE, ct);
        }
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        req.send()
            .await
            .map_err(|e| Error::Upstream(format!("{e}")))
    }

    /// The session document; None on 404 (deleted).
    pub async fn get_session(&self, session_id: &str) -> Result<Option<Value>> {
        let resp = self
            .forward(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}"),
                None,
                None,
                None,
            )
            .await?;
        match resp.status().as_u16() {
            404 => Ok(None),
            s if (200..300).contains(&s) => resp
                .json()
                .await
                .map(Some)
                .map_err(|e| Error::Upstream(format!("session body: {e}"))),
            s => Err(Error::Upstream(format!("GET session -> {s}"))),
        }
    }

    /// Replay the journal after `after`: the durable events, parsed out of the SSE framing.
    pub async fn replay_events(&self, session_id: &str, after: i64) -> Result<Vec<Value>> {
        let resp = self
            .forward(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}/events?after={after}&follow=false"),
                None,
                None,
                None,
            )
            .await?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Ok(Vec::new());
        }
        if !(200..300).contains(&status) {
            return Err(Error::Upstream(format!("GET events -> {status}")));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| Error::Upstream(format!("events body: {e}")))?;
        Ok(parse_sse_data(&text))
    }
}

/// Every `data:` payload in an SSE stream, JSON-parsed; non-JSON payloads are skipped.
pub fn parse_sse_data(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("data:")?;
            serde_json::from_str(rest.trim_start()).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parse_takes_data_lines_only() {
        let text = "id: 3\nevent: turn.started\ndata: {\"type\":\"turn.started\",\"seq\":3}\n\n\
                    : keep-alive\nid: 4\nevent: x\ndata: not-json\n\ndata:{\"seq\":4}\n";
        let events = parse_sse_data(text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["seq"], 3);
        assert_eq!(events[1]["seq"], 4);
    }
}
