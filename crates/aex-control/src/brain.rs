//! The brain, seen from the control plane: a session/v1 server we hold the operator token for.
//!
//! Two uses: verbatim forwarding of customer requests (the authority proxy), and reading the
//! journal for billing — `GET /events?follow=false` replay (the journal IS the SSE event log)
//! plus the session document as projection-repair evidence.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use crate::customer_environment::GatewayRequest;

use crate::{Error, Result};

#[derive(Clone)]
pub struct BrainClient {
    http: reqwest::Client,
    base: String,
    token: String,
    discovery_overlap_ms: i64,
    discovery_session_limit: usize,
}

/// The authoritative, tenant-indexed projection used by metering discovery. `last_seq` is the
/// journal high-water, not merely the highest public SSE event (sequence gaps are expected).
#[derive(Debug, Clone)]
pub struct BrainSessionSnapshot {
    pub id: String,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub depth: i64,
    pub shape: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub last_seq: i64,
    pub document: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainDeletionStatus {
    pub state: String,
    pub completed_at_ms: Option<i64>,
}

const DISCOVERY_STATES: [&str; 5] = ["open", "ending", "ended", "deleting", "failed"];
const CUSTOMER_ENVIRONMENT_HOP_TIMEOUT: Duration = Duration::from_secs(15);
/// Every bounded (non-streaming) Brain call carries this total deadline: a stalled
/// established connection must not hang the sweeper, deletion worker, or /v1/balance.
/// The SSE follow forward is the one exemption (it streams indefinitely by design).
const BRAIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl BrainClient {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        let mut base = base.into();
        while base.ends_with('/') {
            base.pop();
        }
        BrainClient {
            http: reqwest::Client::builder()
                // This client carries the Brain operator credential. The internal control-plane
                // route must never inherit a workstation/container HTTP_PROXY and disclose that
                // bearer or move name resolution outside the service network.
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()
                .expect("internal Brain HTTP client"),
            base,
            token: token.into(),
            discovery_overlap_ms: 120_000,
            discovery_session_limit: 100_000,
        }
    }

    /// Configure incremental tenant-index discovery. The high limit is a corruption/runaway
    /// guard for the one-time bootstrap too; ordinary sweeps stop at the persisted watermark.
    pub fn with_discovery_policy(mut self, overlap_ms: i64, session_limit: usize) -> Self {
        self.discovery_overlap_ms = overlap_ms.max(1);
        self.discovery_session_limit = session_limit.max(1);
        self
    }

    pub fn discovery_overlap_ms(&self) -> i64 {
        self.discovery_overlap_ms
    }

    pub fn discovery_session_limit(&self) -> usize {
        self.discovery_session_limit
    }

    /// Forward a request as the operator. `path_and_query` starts with `/v1/...`.
    // One argument per independent request dimension; a builder here would be ceremony.
    #[allow(clippy::too_many_arguments)]
    pub async fn forward(
        &self,
        tenant_id: &str,
        method: reqwest::Method,
        path_and_query: &str,
        content_type: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<reqwest::Body>,
        deadline: Option<Duration>,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .request(method, format!("{}{path_and_query}", self.base))
            .bearer_auth(&self.token)
            .header("x-brain-tenant-id", tenant_id);
        if let Some(ct) = content_type {
            req = req.header(reqwest::header::CONTENT_TYPE, ct);
        }
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        if let Some(deadline) = deadline {
            req = req.timeout(deadline);
        }
        req.send()
            .await
            .map_err(|e| Error::Upstream(format!("{e}")))
    }

    /// Mint a short-lived customer-environment connection grant under Aex's authenticated tenant.
    pub async fn customer_environment_grant(
        &self,
        tenant_id: &str,
        content_type: Option<&str>,
        body: bytes::Bytes,
    ) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .post(format!(
                "{}/internal/v1/customer-environment/grants",
                self.base
            ))
            .timeout(CUSTOMER_ENVIRONMENT_HOP_TIMEOUT)
            .bearer_auth(&self.token)
            .header("x-brain-tenant-id", tenant_id)
            .body(body);
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        request
            .send()
            .await
            .map_err(|error| Error::Upstream(format!("customer-environment grant: {error}")))
    }

    /// Forward one authenticated API Gateway event without interpreting Brain's frame protocol.
    pub async fn customer_environment_gateway(
        &self,
        metadata: &GatewayRequest,
        content_type: Option<&str>,
        body: bytes::Bytes,
    ) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .post(format!(
                "{}/internal/v1/customer-environment/gateway",
                self.base
            ))
            .timeout(CUSTOMER_ENVIRONMENT_HOP_TIMEOUT)
            .bearer_auth(&self.token)
            .header("x-brain-connection-id", &metadata.connection_id)
            .header("x-brain-route-key", metadata.route.as_brain_header())
            .header("x-brain-request-id", &metadata.request_id)
            .header("x-brain-source-ip", metadata.source_ip.to_string())
            .body(body);
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        if let Some(protocol) = &metadata.protocol {
            request = request.header(reqwest::header::SEC_WEBSOCKET_PROTOCOL, protocol);
        }
        request
            .send()
            .await
            .map_err(|error| Error::Upstream(format!("customer-environment gateway: {error}")))
    }

    /// Forward a terminal operation observation using both trust layers: the Aex-to-Brain
    /// operator bearer authenticates this private route, while the short-lived scoped grant is a
    /// separate header paired with the non-secret path ID. Aex does not interpret the neutral
    /// observation body and never sends a customer API key to Brain.
    pub async fn customer_environment_observation(
        &self,
        grant_id: &str,
        grant: &str,
        content_type: Option<&str>,
        body: bytes::Bytes,
    ) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .post(format!(
                "{}/internal/v1/customer-environment/observations/{grant_id}",
                self.base
            ))
            .timeout(CUSTOMER_ENVIRONMENT_HOP_TIMEOUT)
            .bearer_auth(&self.token)
            .header("x-brain-observation-grant", grant)
            .body(body);
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        request
            .send()
            .await
            .map_err(|error| Error::Upstream(format!("customer-environment observation: {error}")))
    }

    /// The session document; None on 404 (deleted).
    pub async fn get_session(&self, tenant_id: &str, session_id: &str) -> Result<Option<Value>> {
        let resp = self
            .forward(
                tenant_id,
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}"),
                None,
                None,
                None,
                Some(BRAIN_REQUEST_TIMEOUT),
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

    /// Request the recursive logical end fence. This is idempotent; callers observe the ordinary
    /// Session projection separately because hosted Brain may complete the tree asynchronously.
    pub async fn request_end(&self, tenant_id: &str, session_id: &str) -> Result<()> {
        let response = self
            .forward(
                tenant_id,
                reqwest::Method::POST,
                &format!("/v1/sessions/{session_id}/end"),
                None,
                None,
                None,
                Some(BRAIN_REQUEST_TIMEOUT),
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Upstream(format!(
                "POST session end -> {}",
                response.status()
            )))
        }
    }

    /// Strongly consistent direct-child adjacency. Destructive settlement recursively walks this
    /// base partition only after the root end fence, never the eventually consistent tenant GSI.
    pub async fn list_direct_children(
        &self,
        tenant_id: &str,
        parent_id: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<BrainSessionSnapshot>, Option<String>)> {
        let mut url = reqwest::Url::parse("https://brain.invalid/v1/sessions")
            .expect("static child-list URL");
        url.set_path(&format!("/v1/sessions/{parent_id}/children"));
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", "100");
            if let Some(cursor) = cursor {
                query.append_pair("cursor", cursor);
            }
        }
        let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        let response = self
            .forward(
                tenant_id,
                reqwest::Method::GET,
                &path,
                None,
                None,
                None,
                Some(BRAIN_REQUEST_TIMEOUT),
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::Upstream(format!(
                "GET direct children -> {}",
                response.status()
            )));
        }
        let page: Value = response
            .json()
            .await
            .map_err(|error| Error::Upstream(format!("direct-child page: {error}")))?;
        let data = page["data"]
            .as_array()
            .ok_or_else(|| Error::Upstream("direct-child page has no data array".into()))?;
        let snapshots = data
            .iter()
            .cloned()
            .map(parse_session_snapshot)
            .collect::<Result<Vec<_>>>()?;
        let next = match page["has_more"].as_bool() {
            Some(true) => Some(
                page["next_cursor"]
                    .as_str()
                    .ok_or_else(|| {
                        Error::Upstream("direct-child page has_more without next_cursor".into())
                    })?
                    .to_owned(),
            ),
            Some(false) => None,
            None => {
                return Err(Error::Upstream(
                    "direct-child page has no has_more boolean".into(),
                ));
            }
        };
        Ok((snapshots, next))
    }

    /// Accept physical subtree deletion after Aex has durably settled the fenced graph. `true`
    /// means Brain's non-content tombstone already confirms completion.
    pub async fn accept_delete(&self, tenant_id: &str, session_id: &str) -> Result<bool> {
        let response = self
            .forward(
                tenant_id,
                reqwest::Method::DELETE,
                &format!("/v1/sessions/{session_id}?queue=true"),
                None,
                None,
                None,
                Some(BRAIN_REQUEST_TIMEOUT),
            )
            .await?;
        match response.status().as_u16() {
            202 => Ok(false),
            204 => Ok(true),
            status => Err(Error::Upstream(format!(
                "DELETE session acceptance -> {status}"
            ))),
        }
    }

    pub async fn deletion_status(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Option<BrainDeletionStatus>> {
        let response = self
            .forward(
                tenant_id,
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}/deletion"),
                None,
                None,
                None,
                Some(BRAIN_REQUEST_TIMEOUT),
            )
            .await?;
        match response.status().as_u16() {
            404 => Ok(None),
            status if (200..300).contains(&status) => {
                let body: Value = response
                    .json()
                    .await
                    .map_err(|error| Error::Upstream(format!("deletion status: {error}")))?;
                let state = body["state"]
                    .as_str()
                    .filter(|state| {
                        matches!(
                            *state,
                            "accepted" | "deleting" | "retrying" | "blocked" | "succeeded"
                        )
                    })
                    .ok_or_else(|| Error::Upstream("deletion status has invalid state".into()))?;
                let completed_at_ms = body["completed_at_ms"].as_i64();
                if state == "succeeded" && completed_at_ms.is_none() {
                    return Err(Error::Upstream(
                        "succeeded deletion status has no completion timestamp".into(),
                    ));
                }
                Ok(Some(BrainDeletionStatus {
                    state: state.to_owned(),
                    completed_at_ms,
                }))
            }
            status => Err(Error::Upstream(format!("GET deletion status -> {status}"))),
        }
    }

    /// Incrementally replay and fold durable journal events after `after`, capped at the captured
    /// authoritative HEAD high-water. Neither the lifetime response nor its event set is retained.
    pub(crate) async fn fold_replay_events(
        &self,
        tenant_id: &str,
        session_id: &str,
        after: i64,
        through: i64,
        state: &mut crate::rating::FoldState,
    ) -> Result<ReplayFoldStats> {
        let resp = self
            .forward(
                tenant_id,
                reqwest::Method::GET,
                &format!(
                    "/v1/sessions/{session_id}/events?after={after}&through={through}&follow=false"
                ),
                None,
                None,
                None,
                // A bounded replay, but its SSE body can be large: a generous wall that
                // still refuses to hang the rating fold forever on a stalled connection.
                Some(Duration::from_secs(600)),
            )
            .await?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Err(Error::Upstream(
                "Brain replay disappeared after its authoritative HEAD was captured".into(),
            ));
        }
        if !(200..300).contains(&status) {
            return Err(Error::Upstream(format!("GET events -> {status}")));
        }
        let mut decoder = ReplaySseDecoder::default();
        let mut stream = resp.bytes_stream();
        // Publish the candidate fold only after the terminal proof. A malformed event or a clean
        // EOF caused by a failed Brain page cannot advance even this in-memory high-water.
        let mut candidate = state.clone();
        let mut events = 0usize;
        let mut completed_through = None;
        let mut previous_event_seq = after;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| Error::Upstream(format!("events body: {error}")))?;
            decoder.feed(&chunk, |event| {
                if event["type"].as_str() == Some("replay.complete") {
                    let proof = event["through_seq"]
                        .as_u64()
                        .and_then(|seq| i64::try_from(seq).ok())
                        .ok_or_else(|| {
                            Error::Upstream("Brain replay completion has invalid high-water".into())
                        })?;
                    if event["session_id"].as_str() != Some(session_id) || proof != through {
                        return Err(Error::Upstream(
                            "Brain replay completion does not match the requested boundary".into(),
                        ));
                    }
                    if completed_through.replace(proof).is_some() {
                        return Err(Error::Upstream(
                            "Brain replay emitted more than one completion proof".into(),
                        ));
                    }
                    return Ok(());
                }
                if completed_through.is_some() {
                    return Err(Error::Upstream(
                        "Brain replay emitted an event after its completion proof".into(),
                    ));
                }
                if event["session_id"].as_str() != Some(session_id) {
                    return Err(Error::Upstream(
                        "Brain replay event does not belong to the requested session".into(),
                    ));
                }
                let seq = event["seq"]
                    .as_u64()
                    .and_then(|seq| i64::try_from(seq).ok())
                    .ok_or_else(|| {
                        Error::Upstream("Brain replay event has invalid sequence".into())
                    })?;
                if seq <= previous_event_seq {
                    return Err(Error::Upstream(format!(
                        "Brain replay event sequence {seq} is not strictly after {previous_event_seq}"
                    )));
                }
                if seq > through {
                    return Err(Error::Upstream(format!(
                        "Brain replay event sequence {seq} exceeds requested high-water {through}"
                    )));
                }
                previous_event_seq = seq;
                crate::rating::fold_events(&mut candidate, std::slice::from_ref(&event))?;
                events = events.saturating_add(1);
                Ok(())
            })?;
        }
        decoder.finish()?;
        if completed_through != Some(through) {
            return Err(Error::Upstream(
                "Brain replay ended without proving its requested high-water".into(),
            ));
        }
        *state = candidate;
        Ok(ReplayFoldStats {
            events,
            max_buffered_bytes: decoder.max_buffered_bytes,
        })
    }

    /// Count resource-bearing root sessions from Brain's tenant/state GSI, stopping as soon as the
    /// account cap is reached. `ending`, `failed`, and `deleting` are not proof that every sandbox
    /// has been released; only a strong `ended` projection or physical deletion releases the
    /// hosted slot. Otherwise create/end/fail churn could accumulate trees still consuming
    /// resources. Children are governed by the sealed root tree policy, not this account root
    /// limit. This is one paginated listing per capacity-bearing state, never one GET per session.
    pub async fn live_root_session_count(&self, tenant_id: &str, stop_at: i64) -> Result<i64> {
        let stop_at = stop_at.max(1);
        let mut root_ids = HashSet::<String>::new();
        let mut scanned = 0usize;
        for state in ["open", "ending", "failed", "deleting"] {
            let mut cursor: Option<String> = None;
            let mut cursors = HashSet::<String>::new();
            loop {
                let mut url = reqwest::Url::parse("https://brain.invalid/v1/sessions")
                    .expect("static tenant-list URL");
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("state", state);
                    // A small account root limit must not force tiny pages through a child-heavy
                    // tenant index. Brain's public list ceiling is 100.
                    query.append_pair("limit", "100");
                    if let Some(cursor) = &cursor {
                        query.append_pair("cursor", cursor);
                    }
                }
                let path = match url.query() {
                    Some(query) => format!("{}?{query}", url.path()),
                    None => url.path().to_owned(),
                };
                let response = self
                    .forward(
                        tenant_id,
                        reqwest::Method::GET,
                        &path,
                        None,
                        None,
                        None,
                        Some(BRAIN_REQUEST_TIMEOUT),
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(Error::Upstream(format!(
                        "GET tenant sessions ({state}) -> {}",
                        response.status()
                    )));
                }
                let page: Value = response
                    .json()
                    .await
                    .map_err(|error| Error::Upstream(format!("tenant session list: {error}")))?;
                let data = page["data"].as_array().ok_or_else(|| {
                    Error::Upstream("tenant session list has no data array".into())
                })?;
                for document in data {
                    scanned = scanned.saturating_add(1);
                    if scanned > self.discovery_session_limit {
                        return Err(Error::Upstream(format!(
                            "tenant root-session count exceeded its {}-session safety bound",
                            self.discovery_session_limit
                        )));
                    }
                    let snapshot = parse_session_snapshot(document.clone())?;
                    if snapshot.parent_id.is_none() {
                        root_ids.insert(snapshot.id);
                    }
                    if i64::try_from(root_ids.len()).unwrap_or(i64::MAX) >= stop_at {
                        return Ok(stop_at);
                    }
                }
                let next = page_next_cursor(&page, "tenant session list")?;
                let Some(next) = next else {
                    break;
                };
                if !cursors.insert(next.clone()) {
                    return Err(Error::Upstream(
                        "tenant session list repeated a pagination cursor".into(),
                    ));
                }
                cursor = Some(next);
            }
        }
        i64::try_from(root_ids.len())
            .map_err(|_| Error::Upstream("tenant root-session count overflowed".into()))
    }

    /// Read the changed window of Brain's reverse-updated tenant/state index. Every non-deleted
    /// billable state is queried separately so a state transition cannot permanently hide a
    /// session. Callers advance their durable watermark only after all returned snapshots have
    /// been settled successfully.
    pub async fn discover_sessions(
        &self,
        tenant_id: &str,
        watermark_ms: i64,
    ) -> Result<Vec<BrainSessionSnapshot>> {
        let threshold_ms = watermark_ms
            .saturating_sub(self.discovery_overlap_ms)
            .max(0);
        let mut snapshots = HashMap::<String, BrainSessionSnapshot>::new();
        let mut scanned = 0usize;

        for state in DISCOVERY_STATES {
            let mut cursor: Option<String> = None;
            let mut cursors = HashSet::new();
            let mut previous_updated_ms: Option<i64> = None;
            loop {
                let mut url = reqwest::Url::parse("https://brain.invalid/v1/sessions")
                    .expect("static tenant-list URL");
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("state", state);
                    query.append_pair("limit", "100");
                    if let Some(cursor) = &cursor {
                        query.append_pair("cursor", cursor);
                    }
                }
                let path = match url.query() {
                    Some(query) => format!("{}?{query}", url.path()),
                    None => url.path().to_owned(),
                };
                let response = self
                    .forward(
                        tenant_id,
                        reqwest::Method::GET,
                        &path,
                        None,
                        None,
                        None,
                        Some(BRAIN_REQUEST_TIMEOUT),
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(Error::Upstream(format!(
                        "GET tenant discovery ({state}) -> {}",
                        response.status()
                    )));
                }
                let page: Value = response
                    .json()
                    .await
                    .map_err(|error| Error::Upstream(format!("tenant discovery page: {error}")))?;
                let data = page["data"].as_array().ok_or_else(|| {
                    Error::Upstream("tenant discovery page has no data array".into())
                })?;
                let mut reached_watermark = false;
                for document in data {
                    scanned = scanned.saturating_add(1);
                    if scanned > self.discovery_session_limit {
                        return Err(Error::Upstream(format!(
                            "tenant discovery exceeded the configured {}-session safety bound",
                            self.discovery_session_limit
                        )));
                    }
                    let snapshot = parse_session_snapshot(document.clone())?;
                    if snapshot.document["state"].as_str() != Some(state) {
                        return Err(Error::Upstream(format!(
                            "tenant discovery state partition {state} returned session {} in state {}",
                            snapshot.id,
                            snapshot.document["state"].as_str().unwrap_or("<missing>")
                        )));
                    }
                    if previous_updated_ms.is_some_and(|previous| snapshot.updated_ms > previous) {
                        return Err(Error::Upstream(format!(
                            "tenant discovery state {state} is not ordered newest-first"
                        )));
                    }
                    previous_updated_ms = Some(snapshot.updated_ms);
                    if snapshot.updated_ms < threshold_ms {
                        reached_watermark = true;
                        break;
                    }
                    match snapshots.get(&snapshot.id) {
                        Some(existing)
                            if (existing.updated_ms, existing.last_seq)
                                >= (snapshot.updated_ms, snapshot.last_seq) => {}
                        _ => {
                            snapshots.insert(snapshot.id.clone(), snapshot);
                        }
                    }
                }
                let next = page_next_cursor(&page, "tenant discovery page")?;
                if reached_watermark {
                    break;
                }
                let Some(next) = next else { break };
                if !cursors.insert(next.clone()) {
                    return Err(Error::Upstream(
                        "tenant discovery repeated a pagination cursor".into(),
                    ));
                }
                cursor = Some(next);
            }
        }

        Ok(snapshots.into_values().collect())
    }
}

/// Validate and project one Brain-owned Session document. Keeping this parser here lets the
/// control plane use Brain's wire without copying its schema into an Aex-owned contract.
pub fn parse_session_snapshot(document: Value) -> Result<BrainSessionSnapshot> {
    let required_string = |field: &str| {
        document[field]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Upstream(format!("session document has no {field}")))
    };
    let id = required_string("id")?;
    let root_id = required_string("root_id")?;
    let created_at = required_string("created_at")?;
    let created_ms = crate::parse_rfc3339_ms(&created_at)
        .ok_or_else(|| Error::Upstream(format!("session {id} has invalid created_at")))?;
    let updated_at = required_string("updated_at")?;
    let updated_ms = crate::parse_rfc3339_ms(&updated_at)
        .ok_or_else(|| Error::Upstream(format!("session {id} has invalid updated_at")))?;
    let last_seq = document["last_seq"]
        .as_u64()
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| Error::Upstream(format!("session {id} has invalid last_seq")))?;
    let parent_id = match document.get("parent_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(parent_id)) => Some(parent_id.clone()),
        Some(_) => {
            return Err(Error::Upstream(format!(
                "session {id} has invalid parent_id"
            )));
        }
    };
    let depth = document["depth"]
        .as_i64()
        .filter(|depth| (0..=8).contains(depth))
        .ok_or_else(|| Error::Upstream(format!("session {id} has invalid depth")))?;
    let state = required_string("state")?;
    if !matches!(
        state.as_str(),
        "open" | "ending" | "ended" | "deleting" | "deleted" | "failed"
    ) {
        return Err(Error::Upstream(format!(
            "session {id} has invalid lifecycle state"
        )));
    }
    let turn_state = required_string("turn_state")?;
    if !matches!(turn_state.as_str(), "idle" | "running") {
        return Err(Error::Upstream(format!(
            "session {id} has invalid turn state"
        )));
    }
    if (depth == 0 && (parent_id.is_some() || root_id != id))
        || (depth > 0 && (parent_id.is_none() || root_id == id))
    {
        return Err(Error::Upstream(format!(
            "session {id} has contradictory root, parent, or depth fields"
        )));
    }
    let shape = required_string("shape")?;
    if shape != "1gb" {
        return Err(Error::Upstream(format!(
            "session {id} has unsupported hosted managed-compute shape {shape}"
        )));
    }
    Ok(BrainSessionSnapshot {
        id,
        root_id,
        parent_id,
        depth,
        shape,
        created_ms,
        updated_ms,
        last_seq,
        document,
    })
}

fn page_next_cursor(page: &Value, context: &str) -> Result<Option<String>> {
    match page["has_more"].as_bool() {
        Some(false) => Ok(None),
        Some(true) => page["next_cursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
            .map(|cursor| Some(cursor.to_owned()))
            .ok_or_else(|| Error::Upstream(format!("{context} has_more without next_cursor"))),
        None => Err(Error::Upstream(format!(
            "{context} has no has_more boolean"
        ))),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplayFoldStats {
    pub events: usize,
    pub max_buffered_bytes: usize,
}

/// Bounded incremental SSE decoder for the trusted Brain replay boundary. The neutral constant is
/// the UTF-8 JSON payload limit and excludes framing; four KiB of derived framing allowance covers
/// `event`, `id`, `data`, CRLF, and generic split-data fields without redefining the payload limit.
#[derive(Debug, Default)]
struct ReplaySseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    data_lines: usize,
    frame_started: bool,
    max_buffered_bytes: usize,
}

impl ReplaySseDecoder {
    fn feed(&mut self, chunk: &[u8], mut on_event: impl FnMut(Value) -> Result<()>) -> Result<()> {
        for &byte in chunk {
            if byte == b'\n' {
                self.process_line(&mut on_event)?;
                continue;
            }
            self.line.push(byte);
            self.observe_bound()?;
        }
        Ok(())
    }

    fn process_line(&mut self, on_event: &mut impl FnMut(Value) -> Result<()>) -> Result<()> {
        let end = self
            .line
            .len()
            .saturating_sub(usize::from(self.line.last() == Some(&b'\r')));
        if end == 0 {
            self.line.clear();
            if self.data_lines > 0 {
                let event: Value = serde_json::from_slice(&self.data).map_err(|error| {
                    Error::Upstream(format!("invalid Brain replay event: {error}"))
                })?;
                validate_replay_event(&event)?;
                on_event(event)?;
            }
            self.data.clear();
            self.data_lines = 0;
            self.frame_started = false;
            return Ok(());
        }

        self.frame_started = true;
        if self.line.first() == Some(&b':') {
            self.line.clear();
            return Ok(());
        }
        let separator = self.line[..end].iter().position(|byte| *byte == b':');
        let is_data = separator.is_some_and(|separator| &self.line[..separator] == b"data");
        if is_data {
            let mut value_start = separator.expect("checked above") + 1;
            if self.line.get(value_start) == Some(&b' ') {
                value_start += 1;
            }
            let value_len = end.saturating_sub(value_start);
            let separator = usize::from(self.data_lines > 0);
            let projected = self
                .data
                .len()
                .saturating_add(separator)
                .saturating_add(value_len);
            if projected > brain_protocol::MAX_PUBLIC_EVENT_BYTES {
                return Err(Error::Upstream(format!(
                    "Brain replay event exceeds {} bytes",
                    brain_protocol::MAX_PUBLIC_EVENT_BYTES
                )));
            }
            if separator == 1 {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(&self.line[value_start..end]);
            self.data_lines = self.data_lines.saturating_add(1);
        }
        self.line.clear();
        self.observe_bound()?;
        Ok(())
    }

    fn observe_bound(&mut self) -> Result<()> {
        let buffered = self.line.len().saturating_add(self.data.len());
        let resident_limit = brain_protocol::MAX_PUBLIC_EVENT_BYTES.saturating_add(4 * 1024);
        if buffered > resident_limit {
            return Err(Error::Upstream(format!(
                "Brain replay SSE frame exceeds its {}-byte payload bound",
                brain_protocol::MAX_PUBLIC_EVENT_BYTES
            )));
        }
        self.max_buffered_bytes = self.max_buffered_bytes.max(buffered);
        Ok(())
    }

    fn finish(&self) -> Result<()> {
        if !self.line.is_empty()
            || !self.data.is_empty()
            || self.data_lines > 0
            || self.frame_started
        {
            return Err(Error::Upstream(
                "Brain replay stream ended in a truncated SSE frame".into(),
            ));
        }
        Ok(())
    }
}

fn validate_replay_event(event: &Value) -> Result<()> {
    let Some(event_type) = event["type"].as_str() else {
        return Err(Error::Upstream(
            "Brain replay event has no valid type or sequence".into(),
        ));
    };
    if event_type == "replay.complete" {
        if event["session_id"].as_str().is_some()
            && event["through_seq"]
                .as_u64()
                .and_then(|seq| i64::try_from(seq).ok())
                .is_some()
        {
            return Ok(());
        }
    } else if event["seq"]
        .as_u64()
        .and_then(|seq| i64::try_from(seq).ok())
        .is_some()
    {
        return Ok(());
    }
    Err(Error::Upstream(
        "Brain replay event has no valid type or sequence".into(),
    ))
}

/// Test helper over the same strict bounded decoder used by HTTP replay.
#[cfg(test)]
fn parse_sse_data(text: &str) -> Result<Vec<Value>> {
    let mut decoder = ReplaySseDecoder::default();
    let mut events = Vec::new();
    decoder.feed(text.as_bytes(), |event| {
        events.push(event);
        Ok(())
    })?;
    decoder.finish()?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::extract::State;
    use axum::http::Uri;
    use axum::response::Response;
    use serde_json::json;

    use super::*;

    #[test]
    fn sse_parse_takes_data_lines_only() {
        let text = "id: 3\nevent: turn.started\ndata: {\"type\":\"turn.started\",\"seq\":3}\n\n\
                    : keep-alive\nid: 4\nevent: model.usage\ndata:{\"type\":\"model.usage\",\"seq\":4}\n\n";
        let events = parse_sse_data(text).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["seq"], 3);
        assert_eq!(events[1]["seq"], 4);
    }

    #[test]
    fn malformed_replay_data_fails_instead_of_skipping_a_billable_record() {
        assert!(parse_sse_data("id: 3\ndata: not-json\n\n").is_err());
        assert!(parse_sse_data("data: {\"type\":\"model.usage\"}\n\n").is_err());
        assert!(
            parse_sse_data("data: {\"type\":\"model.usage\",\"seq\":3}\n").is_err(),
            "EOF must not dispatch a frame without its blank delimiter"
        );
        assert!(
            parse_sse_data(&"x".repeat(brain_protocol::MAX_PUBLIC_EVENT_BYTES + 4 * 1024 + 1))
                .is_err(),
            "a never-terminated line stays bounded"
        );
    }

    #[test]
    fn public_event_payload_bound_is_exact_and_fragmentation_stays_constant_space() {
        let fixed = json!({"type":"model.usage","seq":1,"padding":""}).to_string();
        let exact = json!({
            "type":"model.usage",
            "seq":1,
            "padding":"x".repeat(brain_protocol::MAX_PUBLIC_EVENT_BYTES - fixed.len())
        })
        .to_string();
        assert_eq!(exact.len(), brain_protocol::MAX_PUBLIC_EVENT_BYTES);

        let mut decoder = ReplaySseDecoder::default();
        let mut seen = 0usize;
        let frame = format!("data:{exact}\n\n");
        decoder
            .feed(&frame.as_bytes()[..1], |_| {
                seen += 1;
                Ok(())
            })
            .unwrap();
        for chunk in frame.as_bytes()[1..].chunks(37) {
            decoder
                .feed(chunk, |_| {
                    seen += 1;
                    Ok(())
                })
                .unwrap();
        }

        // Simulate many paged/yielded replay chunks without retaining their lifetime journal.
        for seq in 2..=20_000 {
            let frame = format!("data:{{\"type\":\"model.usage\",\"seq\":{seq}}}\n\n");
            for chunk in frame.as_bytes().chunks(11) {
                decoder
                    .feed(chunk, |_| {
                        seen += 1;
                        Ok(())
                    })
                    .unwrap();
            }
        }
        decoder.finish().unwrap();
        assert_eq!(seen, 20_000);
        assert!(
            decoder.max_buffered_bytes
                <= brain_protocol::MAX_PUBLIC_EVENT_BYTES.saturating_add(4 * 1024)
        );

        let over = json!({
            "type":"model.usage",
            "seq":1,
            "padding":"x".repeat(brain_protocol::MAX_PUBLIC_EVENT_BYTES + 1 - fixed.len())
        })
        .to_string();
        assert_eq!(over.len(), brain_protocol::MAX_PUBLIC_EVENT_BYTES + 1);
        assert!(parse_sse_data(&format!("data:{over}\n\n")).is_err());
    }

    async fn paged_replay_handler(
        State(chunks): State<Arc<Vec<bytes::Bytes>>>,
        uri: Uri,
    ) -> Response {
        if uri.path().ends_with("/events") {
            let body = Body::from_stream(futures_util::stream::iter(
                chunks
                    .as_ref()
                    .clone()
                    .into_iter()
                    .map(Ok::<bytes::Bytes, Infallible>),
            ));
            return Response::builder()
                .status(200)
                .header(reqwest::header::CONTENT_TYPE, "text/event-stream")
                .body(body)
                .unwrap();
        }
        Response::builder().status(404).body(Body::empty()).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_replay_folds_many_yielded_pages_without_a_lifetime_buffer() {
        let mut wire = String::new();
        for seq in 1..=5_000 {
            wire.push_str(&format!(
                "data:{{\"type\":\"model.usage\",\"seq\":{seq},\"session_id\":\"ses_many\"}}\n\n"
            ));
        }
        wire.push_str(
            "data:{\"type\":\"replay.complete\",\"session_id\":\"ses_many\",\"through_seq\":5000}\n\n",
        );
        let mut chunks = vec![bytes::Bytes::copy_from_slice(&wire.as_bytes()[..1])];
        chunks.extend(
            wire.as_bytes()[1..]
                .chunks(127)
                .map(bytes::Bytes::copy_from_slice),
        );
        let app = axum::Router::new()
            .fallback(paged_replay_handler)
            .with_state(Arc::new(chunks));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut fold = crate::rating::FoldState::default();
        let stats = BrainClient::new(base, "operator")
            .fold_replay_events("acc", "ses_many", 0, 5_000, &mut fold)
            .await
            .unwrap();
        assert_eq!(stats.events, 5_000);
        assert_eq!(fold.folded_seq, 5_000);
        assert!(
            stats.max_buffered_bytes < 256,
            "resident replay memory follows one event, not 5,000-event history: {:?}",
            stats
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clean_eof_without_completion_proof_cannot_advance_the_fold() {
        let chunks = Arc::new(vec![bytes::Bytes::from_static(
            b"data:{\"type\":\"model.usage\",\"seq\":1,\"session_id\":\"ses_many\"}\n\n",
        )]);
        let app = axum::Router::new()
            .fallback(paged_replay_handler)
            .with_state(chunks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut fold = crate::rating::FoldState::default();
        let error = BrainClient::new(base, "operator")
            .fold_replay_events("acc", "ses_many", 0, 1, &mut fold)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("without proving"), "{error}");
        assert_eq!(fold, crate::rating::FoldState::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replay_rejects_events_beyond_its_captured_high_water() {
        let chunks = Arc::new(vec![bytes::Bytes::from_static(
            b"data:{\"type\":\"model.usage\",\"seq\":2,\"session_id\":\"ses_many\"}\n\n\
              data:{\"type\":\"replay.complete\",\"session_id\":\"ses_many\",\"through_seq\":1}\n\n",
        )]);
        let app = axum::Router::new()
            .fallback(paged_replay_handler)
            .with_state(chunks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut fold = crate::rating::FoldState::default();
        let error = BrainClient::new(base, "operator")
            .fold_replay_events("acc", "ses_many", 0, 1, &mut fold)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exceeds requested high-water"));
        assert_eq!(fold, crate::rating::FoldState::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replay_rejects_duplicate_or_out_of_order_public_sequences() {
        let chunks = Arc::new(vec![bytes::Bytes::from_static(
            b"data:{\"type\":\"model.usage\",\"seq\":2,\"session_id\":\"ses_many\"}\n\n\
              data:{\"type\":\"model.usage\",\"seq\":1,\"session_id\":\"ses_many\"}\n\n\
              data:{\"type\":\"replay.complete\",\"session_id\":\"ses_many\",\"through_seq\":2}\n\n",
        )]);
        let app = axum::Router::new()
            .fallback(paged_replay_handler)
            .with_state(chunks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut fold = crate::rating::FoldState::default();
        let error = BrainClient::new(base, "operator")
            .fold_replay_events("acc", "ses_many", 0, 2, &mut fold)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not strictly after"));
        assert_eq!(fold, crate::rating::FoldState::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replay_rejects_an_event_for_another_session() {
        let chunks = Arc::new(vec![bytes::Bytes::from_static(
            b"data:{\"type\":\"model.usage\",\"seq\":1,\"session_id\":\"ses_other\"}\n\n\
              data:{\"type\":\"replay.complete\",\"session_id\":\"ses_many\",\"through_seq\":1}\n\n",
        )]);
        let app = axum::Router::new()
            .fallback(paged_replay_handler)
            .with_state(chunks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut fold = crate::rating::FoldState::default();
        let error = BrainClient::new(base, "operator")
            .fold_replay_events("acc", "ses_many", 0, 1, &mut fold)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requested session"), "{error}");
        assert_eq!(fold, crate::rating::FoldState::default());
    }

    #[test]
    fn paginated_session_lists_require_an_explicit_boolean_and_nonempty_cursor() {
        assert!(page_next_cursor(&json!({}), "page").is_err());
        assert!(page_next_cursor(&json!({"has_more":"yes"}), "page").is_err());
        assert!(page_next_cursor(&json!({"has_more":true}), "page").is_err());
        assert!(page_next_cursor(&json!({"has_more":true,"next_cursor":""}), "page").is_err());
        assert_eq!(
            page_next_cursor(&json!({"has_more":true,"next_cursor":"next"}), "page").unwrap(),
            Some("next".into())
        );
        assert_eq!(
            page_next_cursor(&json!({"has_more":false}), "page").unwrap(),
            None
        );
    }

    #[test]
    fn session_snapshot_keeps_lifecycle_turn_and_tree_axes_distinct() {
        let root = json!({
            "id":"ses_root",
            "root_id":"ses_root",
            "depth":0,
            "state":"open",
            "turn_state":"running",
            "shape":"1gb",
            "created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-01T00:00:01Z",
            "last_seq":1
        });
        assert_eq!(parse_session_snapshot(root.clone()).unwrap().shape, "1gb");

        let mutate = |field: &str, value: Value| {
            let mut document = root.clone();
            document[field] = value;
            document
        };
        let mut child_without_root = mutate("depth", json!(1));
        child_without_root["parent_id"] = json!("ses_parent");
        for invalid in [
            mutate("state", json!("idle")),
            mutate("turn_state", json!("completed")),
            mutate("shape", json!("2gb")),
            child_without_root,
            mutate("parent_id", json!("ses_parent")),
        ] {
            assert!(parse_session_snapshot(invalid).is_err());
        }

        let mut child = root;
        child["id"] = json!("ses_child");
        child["parent_id"] = json!("ses_root");
        child["depth"] = json!(1);
        child["turn_state"] = json!("idle");
        let mut wrong_child_shape = child.clone();
        wrong_child_shape["shape"] = json!("2gb");
        assert!(
            parse_session_snapshot(wrong_child_shape).is_err(),
            "a hidden child cannot escape the hosted root's 1gb physical seal",
        );
        assert!(parse_session_snapshot(child).is_ok());
    }

    async fn transition_list_handler(
        State(open_queries): State<Arc<AtomicUsize>>,
        uri: Uri,
    ) -> Response {
        let state = uri.query().and_then(|query| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("state="))
        });
        let data = if state == Some("open") && open_queries.fetch_add(1, Ordering::SeqCst) > 0 {
            vec![json!({
                "id": "ses_transition000000000001",
                "root_id": "ses_transition000000000001",
                "depth": 0,
                "object": "session",
                "state": "open",
                "turn_state": "idle",
                "shape": "1gb",
                "model": {"provider":"anthropic", "name":"model"},
                "storage": {"session_storage_bytes":0, "upload_reserved_bytes":0},
                "created_at": crate::rfc3339(crate::now_ms() - 1_000),
                "updated_at": crate::rfc3339(crate::now_ms()),
                "last_seq": 2,
                "turns": 1,
                "metadata": {}
            })]
        } else {
            Vec::new()
        };
        Response::builder()
            .status(200)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"object":"list", "data":data, "has_more":false}).to_string(),
            ))
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overlap_recovers_a_state_transition_missed_between_partition_queries() {
        let open_queries = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .fallback(transition_list_handler)
            .with_state(open_queries);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let brain = BrainClient::new(base, "operator").with_discovery_policy(120_000, 100);
        let watermark = crate::now_ms();
        assert!(
            brain
                .discover_sessions("acc", watermark)
                .await
                .unwrap()
                .is_empty()
        );
        let recovered = brain.discover_sessions("acc", watermark).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "ses_transition000000000001");
    }

    fn listed_session(id: &str, parent_id: Option<&str>, state: &str) -> Value {
        let root_id = if parent_id.is_some() { "ses_root" } else { id };
        let mut document = json!({
            "id": id,
            "root_id": root_id,
            "depth": usize::from(parent_id.is_some()),
            "object": "session",
            "state": state,
            "turn_state": "idle",
            "shape": "1gb",
            "model": {"provider":"anthropic", "name":"model"},
            "storage": {"session_storage_bytes":0, "upload_reserved_bytes":0},
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:01Z",
            "last_seq": 1,
            "turns": 0,
            "metadata": {}
        });
        if let Some(parent_id) = parent_id {
            document["parent_id"] = Value::String(parent_id.to_owned());
        }
        document
    }

    async fn root_count_handler(
        State(requested_limits): State<Arc<Mutex<Vec<usize>>>>,
        uri: Uri,
    ) -> Response {
        let url =
            reqwest::Url::parse(&format!("http://brain.test{uri}")).expect("test request URI");
        let query = url.query_pairs().collect::<HashMap<_, _>>();
        let state = query.get("state").map(|value| value.as_ref()).unwrap_or("");
        let limit = query
            .get("limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        requested_limits.lock().unwrap().push(limit);
        let offset = query
            .get("cursor")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);

        let all = if state == "open" {
            let mut sessions = (0..100)
                .map(|index| listed_session(&format!("ses_child_{index}"), Some("ses_root"), state))
                .collect::<Vec<_>>();
            sessions.push(listed_session("ses_root", None, state));
            sessions
        } else if state == "ending" {
            vec![
                // An eventually consistent state transition may be visible in both partitions;
                // root identity, not rows scanned, is the account-cap authority.
                listed_session("ses_root", None, state),
                listed_session("ses_ending", None, state),
            ]
        } else if state == "failed" {
            vec![listed_session("ses_failed", None, state)]
        } else if state == "deleting" {
            vec![listed_session("ses_deleting", None, state)]
        } else {
            Vec::new()
        };
        let data = all
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next = offset.saturating_add(data.len());
        let has_more = next < all.len();
        Response::builder()
            .status(200)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "object":"list",
                    "data":data,
                    "has_more":has_more,
                    "next_cursor":has_more.then_some(next.to_string())
                })
                .to_string(),
            ))
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn account_concurrency_counts_every_resource_bearing_root_only_once() {
        let requested_limits = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .fallback(root_count_handler)
            .with_state(requested_limits.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let count = BrainClient::new(base, "operator")
            .with_discovery_policy(120_000, 500)
            .live_root_session_count("acc", 10)
            .await
            .unwrap();
        assert_eq!(
            count, 4,
            "children consume no slots; ending, failed, and deleting roots retain capacity; and a cross-state duplicate counts once"
        );
        assert!(
            requested_limits
                .lock()
                .unwrap()
                .iter()
                .all(|limit| *limit == 100),
            "a small root limit must not produce tiny pages through child-heavy tenants"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn root_count_fails_closed_at_the_tenant_scan_safety_bound() {
        let app = axum::Router::new()
            .fallback(root_count_handler)
            .with_state(Arc::new(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let error = BrainClient::new(base, "operator")
            .with_discovery_policy(120_000, 50)
            .live_root_session_count("acc", 10)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("safety bound"), "{error}");
    }

    async fn repeated_root_count_cursor_handler() -> Response {
        Response::builder()
            .status(200)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "object":"list",
                    "data":[listed_session("ses_child", Some("ses_root"), "open")],
                    "has_more":true,
                    "next_cursor":"same"
                })
                .to_string(),
            ))
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn root_count_rejects_a_repeated_pagination_cursor() {
        let app = axum::Router::new().fallback(repeated_root_count_cursor_handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let error = BrainClient::new(base, "operator")
            .with_discovery_policy(120_000, 500)
            .live_root_session_count("acc", 10)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("repeated"), "{error}");
    }
}
