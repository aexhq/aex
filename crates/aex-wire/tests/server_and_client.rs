//! The generated server traits, the dispatch surface and the low-level client.
//!
//! These are behaviour assertions, not presence assertions: coverage of all 32
//! operations is a generator gate (`aex-contract-gen`'s `surface` suite), and
//! this suite proves that what is generated actually decodes strictly, answers
//! with the status the route declares, and refuses a code the route does not.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use aex_wire::client::{
    BaseUrl, ClientError, HEADER_ACCEPT, HEADER_CONTENT_TYPE, HEADER_IDEMPOTENCY_KEY,
    HEADER_IF_MATCH, HEADER_OPERATION_ID, PathWriter, QueryWriter, ToParam, Transport,
    TransportError, WireClient, WireRequest, WireResponse, api_key_revoke_request,
    api_keys_list_request, percent_encode, session_terminate_request, sessions_list_request,
};
use aex_wire::dispatch::{
    DispatchOutcome, FromParam, QueryReader, RawRequest, RequestIdentity, RequestLimits,
    canonical_intent, percent_decode, request_identity,
};
use aex_wire::error::{ErrorCode, ErrorDetails, WireError, WireResult};
use aex_wire::idempotency::{IdempotencyKey, IdempotencyKind, PrincipalScope};
use aex_wire::ids::{
    ApiKeyId, OperationId, OrganizationId, PrefixedId, SessionId, UserId, WorkspaceId,
};
use aex_wire::limits::LimitId;
use aex_wire::models::{
    ApiKeyCreateRequest, ApiKeyPage, BillingUsageCategory, EmptyRequest, MessagePage,
    MessageSendRequest, MessageSendResult, NewApiKey, Session, SessionCommandReceipt,
    SessionCreateRequest, SessionListItem, SessionListPage, SessionMessagesListQuery,
    SessionMessagesStreamQuery, SessionStatus, SessionTelemetryReplayQuery,
    SessionTelemetryStreamQuery, SessionsListQuery, TelemetryDownloadGrant,
    TelemetryDownloadRequest,
};
use aex_wire::provider::ProviderId;
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::server::{
    ApiKeysApi, Created, NdjsonStream, NoContent, RequestContext, RouteGroup, SessionsApi, WithETag,
    dispatch_api_keys, dispatch_sessions,
};
use aex_wire::types::{ETag, HttpMethod, RequestId, Timestamp};

// ---------------------------------------------------------------------------
// A dependency-free executor
// ---------------------------------------------------------------------------

/// Drives a future to completion on the calling thread.
///
/// `aex-wire` has no async runtime and must not gain one, and every future in
/// this suite is immediately ready, so a busy poll is both sufficient and
/// honest about what it is.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A fixed, valid `UUIDv7` Crockford suffix.
const SUFFIX: &str = "01kyw2qa4ne00r40r40m30e209";

fn api_key_id() -> ApiKeyId {
    ApiKeyId::parse(&format!("key_{SUFFIX}")).expect("api key id")
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse(&format!("wsp_{SUFFIX}")).expect("workspace id")
}

fn organization_id() -> OrganizationId {
    OrganizationId::parse(&format!("org_{SUFFIX}")).expect("organization id")
}

fn operation_id() -> OperationId {
    OperationId::parse(&format!("op_{SUFFIX}")).expect("operation id")
}

fn session_id() -> SessionId {
    SessionId::parse(&format!("ses_{SUFFIX}")).expect("session id")
}

/// One row a session list answers with.
fn session_list_item() -> SessionListItem {
    serde_json::from_value(serde_json::json!({
        "createdAt": "2026-08-01T00:00:00.000Z",
        "expiresAt": "2026-08-01T08:00:00.000Z",
        "id": format!("ses_{SUFFIX}"),
        "model": "gpt-4.1-mini",
        "provider": "openai",
        "sandboxStatus": "suspended",
        "status": "idle",
        "updatedAt": "2026-08-01T00:00:00.000Z",
        "workspaceId": format!("wsp_{SUFFIX}"),
    }))
    .expect("session fixture matches the generated model")
}

/// The receipt a session command admission answers with.
fn receipt() -> SessionCommandReceipt {
    serde_json::from_value(serde_json::json!({
        "acceptedAt": "2026-08-01T00:00:00.000Z",
        "operationId": format!("op_{SUFFIX}"),
        "sessionId": format!("ses_{SUFFIX}"),
    }))
    .expect("receipt fixture matches the generated model")
}

fn context(id: RouteId) -> RequestContext {
    RequestContext {
        request_id: RequestId::parse("req-0001").expect("request id"),
        route: id,
        principal: PrincipalScope::Account {
            user: UserId::parse(&format!("usr_{SUFFIX}")).expect("user id"),
            organization: Some(organization_id()),
        },
        actor_session_id: None,
        granted_scopes: ScopeSet::empty(),
        idempotency_key: None,
        operation_id: None,
        if_match: None,
        accept: aex_wire::server::AcceptKind::Json,
    }
}

fn raw<'a>(id: RouteId, path: &'a str, query: &'a str, body: &'a [u8]) -> RawRequest<'a> {
    let descriptor = route(id);
    let (matched, binding) =
        match_route(descriptor.plane, descriptor.method, path).expect("the sample path matches");
    assert_eq!(matched, id, "the sample path resolved to another route");
    RawRequest {
        route: id,
        path: binding,
        query,
        body,
    }
}

/// A stub that answers every route of its group.
struct Stub {
    /// What the handler answers with instead of a success.
    failure: Option<ErrorCode>,
}

impl Stub {
    const fn ok() -> Self {
        Self { failure: None }
    }

    const fn failing(code: ErrorCode) -> Self {
        Self {
            failure: Some(code),
        }
    }

    fn check<T>(&self, value: T) -> WireResult<T> {
        match self.failure {
            Some(code) => Err(WireError::new(code)),
            None => Ok(value),
        }
    }
}

impl ApiKeysApi for Stub {
    async fn api_key_create(
        &self,
        _cx: &RequestContext,
        body: ApiKeyCreateRequest,
    ) -> WireResult<Created<NewApiKey>> {
        self.check(Created(NewApiKey {
            created_at: Timestamp::parse("2026-08-01T00:00:00.000Z").expect("timestamp"),
            id: api_key_id(),
            name: body.name,
            scopes: body.scopes,
            value: "aex_wk_euw1_secret".to_owned(),
            workspace_id: body.workspace_id,
        }))
    }

    async fn api_key_revoke(
        &self,
        _cx: &RequestContext,
        _api_key_id: ApiKeyId,
    ) -> WireResult<NoContent> {
        self.check(NoContent)
    }

    async fn api_keys_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::ApiKeysListQuery,
    ) -> WireResult<ApiKeyPage> {
        self.check(ApiKeyPage {
            items: Vec::new(),
            next_cursor: None,
        })
    }
}

impl SessionsApi for Stub {
    type FrameStream = ();

    async fn session_cancel(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _body: EmptyRequest,
    ) -> WireResult<SessionCommandReceipt> {
        self.check(receipt())
    }

    async fn session_create(
        &self,
        _cx: &RequestContext,
        _body: SessionCreateRequest,
    ) -> WireResult<Created<Session>> {
        unreachable!("the session-create dispatch is not exercised by this suite")
    }

    async fn session_delete(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _body: EmptyRequest,
    ) -> WireResult<SessionCommandReceipt> {
        self.check(receipt())
    }

    async fn session_get(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
    ) -> WireResult<WithETag<Session>> {
        unreachable!("the session-get dispatch is not exercised by this suite")
    }

    async fn session_message_send(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _body: MessageSendRequest,
    ) -> WireResult<Created<MessageSendResult>> {
        unreachable!("the message-send dispatch is not exercised by this suite")
    }

    async fn session_messages_list(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _query: SessionMessagesListQuery,
    ) -> WireResult<MessagePage> {
        self.check(MessagePage {
            items: Vec::new(),
            next_cursor: None,
        })
    }

    async fn session_messages_stream(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _query: SessionMessagesStreamQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.check(NdjsonStream(()))
    }

    async fn session_telemetry_download_create(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _body: TelemetryDownloadRequest,
    ) -> WireResult<Created<TelemetryDownloadGrant>> {
        unreachable!("the telemetry-download dispatch is not exercised by this suite")
    }

    async fn session_telemetry_replay(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _query: SessionTelemetryReplayQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.check(NdjsonStream(()))
    }

    async fn session_telemetry_stream(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _query: SessionTelemetryStreamQuery,
    ) -> WireResult<NdjsonStream<Self::FrameStream>> {
        self.check(NdjsonStream(()))
    }

    async fn session_terminate(
        &self,
        _cx: &RequestContext,
        _session_id: SessionId,
        _body: EmptyRequest,
    ) -> WireResult<SessionCommandReceipt> {
        self.check(receipt())
    }

    async fn sessions_list(
        &self,
        _cx: &RequestContext,
        _query: SessionsListQuery,
    ) -> WireResult<SessionListPage> {
        self.check(SessionListPage {
            items: vec![session_list_item()],
            next_cursor: None,
        })
    }
}

// ---------------------------------------------------------------------------
// The group projection
// ---------------------------------------------------------------------------

#[test]
fn route_groups_partition_the_route_table() {
    let mut seen: Vec<RouteId> = Vec::new();
    for group in RouteGroup::ALL {
        let routes = group.routes();
        assert!(!routes.is_empty(), "`{}` mounts nothing", group.as_str());
        for id in routes {
            assert_eq!(
                id.group(),
                *group,
                "`{}` is listed under `{}` but reports another group",
                id.as_str(),
                group.as_str()
            );
            assert_eq!(
                route(*id).plane,
                group.plane(),
                "`{}` is on the wrong plane for `{}`",
                id.as_str(),
                group.as_str()
            );
            seen.push(*id);
        }
    }
    seen.sort_unstable();
    let mut deduped = seen.clone();
    deduped.dedup();
    assert_eq!(seen, deduped, "a route is mounted by two groups");
    assert_eq!(
        seen.len(),
        RouteId::ALL.len(),
        "the groups do not cover every route"
    );
    assert_eq!(seen, RouteId::ALL.to_vec());
}

#[test]
fn every_group_names_a_distinct_trait_and_key() {
    let mut keys: Vec<&str> = RouteGroup::ALL.iter().map(|g| g.as_str()).collect();
    let mut traits: Vec<&str> = RouteGroup::ALL.iter().map(|g| g.trait_name()).collect();
    keys.sort_unstable();
    traits.sort_unstable();
    let unique_keys = {
        let mut copy = keys.clone();
        copy.dedup();
        copy
    };
    let unique_traits = {
        let mut copy = traits.clone();
        copy.dedup();
        copy
    };
    assert_eq!(keys, unique_keys, "two groups share a key");
    assert_eq!(traits, unique_traits, "two groups share a trait name");
}

#[test]
fn the_surface_corpus_agrees_with_the_route_table() {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        operation_id: String,
        route_id: String,
        group: String,
        r#trait: String,
        success_status: u16,
        transport: String,
        idempotency: String,
    }

    let rows: Vec<Row> = aex_wire::testing::corpus::read_jsonl("routes/surface.jsonl");
    assert_eq!(rows.len(), RouteId::ALL.len());
    let mut keys: std::collections::BTreeMap<&str, RouteGroup> = std::collections::BTreeMap::new();
    for row in &rows {
        let id = RouteId::parse(&row.operation_id)
            .unwrap_or_else(|| panic!("`{}` is not a route", row.operation_id));
        let descriptor = route(id);
        assert_eq!(descriptor.success_status, row.success_status);
        assert_eq!(
            format!("{:?}", descriptor.transport).to_lowercase(),
            row.transport
        );
        assert_eq!(
            id.group().trait_name(),
            row.r#trait,
            "`{}`",
            row.operation_id
        );
        assert!(
            row.route_id.chars().next().is_some_and(char::is_uppercase),
            "`{}` is not a PascalCase RouteId",
            row.route_id
        );
        let expected = match descriptor.idempotency {
            IdempotencyKind::None => "none",
            IdempotencyKind::IdempotencyKey => "idempotency_key",
            IdempotencyKind::OperationId => "operation_id",
        };
        assert_eq!(expected, row.idempotency, "`{}`", row.operation_id);
        // One corpus group key names exactly one generated group, in both
        // directions: two keys mapping to one group would hide a collision.
        let bound = keys.entry(row.group.as_str()).or_insert_with(|| id.group());
        assert_eq!(*bound, id.group(), "`{}` spans two groups", row.group);
    }
    assert_eq!(keys.len(), RouteGroup::ALL.len());
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[test]
fn a_created_route_answers_with_its_declared_status_and_body() {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "ci",
        "scopes": ["sessions:read"],
        "workspaceId": format!("wsp_{SUFFIX}"),
    }))
    .expect("body");
    let request = raw(RouteId::ApiKeyCreate, "/api/api-keys", "", &body);
    let outcome = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyCreate),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect("dispatch");
    let response = outcome.into_unary().expect("a unary answer");
    assert_eq!(response.status, 201);
    assert_eq!(response.content_type, Some("application/json"));
    assert!(response.location.is_none());
    let decoded: NewApiKey = serde_json::from_slice(&response.body).expect("body");
    assert_eq!(decoded.name, "ci");
}

#[test]
fn a_bodyless_route_answers_with_204_and_no_body() {
    let path = format!("/api/api-keys/key_{SUFFIX}");
    let request = raw(RouteId::ApiKeyRevoke, &path, "", b"");
    let outcome = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyRevoke),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect("dispatch");
    let response = outcome.into_unary().expect("a unary answer");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
    assert!(response.content_type.is_none());
}

#[test]
fn a_command_admission_answers_202_with_its_receipt() {
    let body = serde_json::to_vec(&serde_json::json!({})).expect("body");
    let path = format!("/api/sessions/ses_{SUFFIX}/terminations");
    let request = raw(RouteId::SessionTerminate, &path, "", &body);
    let mut cx = context(RouteId::SessionTerminate);
    cx.operation_id = Some(operation_id());
    let outcome = block_on(dispatch_sessions(
        &Stub::ok(),
        &cx,
        request,
        RequestLimits::DEFAULT,
    ))
    .expect("dispatch");
    let response = outcome.into_unary().expect("a unary answer");
    assert_eq!(response.status, 202);
    assert!(response.location.is_none());
    let decoded: SessionCommandReceipt = serde_json::from_slice(&response.body).expect("body");
    assert_eq!(decoded, receipt());
}

#[test]
fn an_unknown_request_member_is_rejected_with_a_typed_validation_detail() {
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "ci",
        "scopes": ["sessions:read"],
        "workspaceId": format!("wsp_{SUFFIX}"),
        "surprise": true,
    }))
    .expect("body");
    let request = raw(RouteId::ApiKeyCreate, "/api/api-keys", "", &body);
    let failure = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyCreate),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("an unknown member is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
    let Some(ErrorDetails::Validation(detail)) = failure.details else {
        panic!("the rejection carried no validation detail");
    };
    assert!(
        detail.reason.contains("surprise"),
        "the reason does not name the offending member: {}",
        detail.reason
    );
}

#[test]
fn a_body_over_the_bound_is_refused_before_it_is_parsed() {
    // Deliberately not valid JSON: a `413` that only appears after a successful
    // parse is not a bound, it is a coincidence.
    let body = vec![b'{'; 128];
    let request = raw(RouteId::ApiKeyCreate, "/api/api-keys", "", &body);
    let limits = RequestLimits {
        max_json_body_bytes: 32,
        max_otlp_body_bytes: 32,
    };
    let failure = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyCreate),
        request,
        limits,
    ))
    .expect_err("an oversize body is a 413");
    assert_eq!(failure.code, ErrorCode::PayloadTooLarge);
}

#[test]
fn a_route_with_no_declared_body_refuses_one() {
    let path = format!("/api/api-keys/key_{SUFFIX}");
    let request = raw(RouteId::ApiKeyRevoke, &path, "", b"{}");
    let failure = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyRevoke),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("a body on a bodyless route is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn an_undeclared_query_parameter_is_rejected() {
    let request = raw(RouteId::SessionsList, "/api/sessions", "surprise=1", b"");
    let failure = block_on(dispatch_sessions(
        &Stub::ok(),
        &context(RouteId::SessionsList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("an unknown query parameter is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_query_parameter_outside_its_declared_bounds_is_rejected() {
    let request = raw(RouteId::SessionsList, "/api/sessions", "limit=100000", b"");
    let failure = block_on(dispatch_sessions(
        &Stub::ok(),
        &context(RouteId::SessionsList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("a limit above the declared maximum is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_repeated_query_parameter_is_rejected() {
    let reader = QueryReader::parse(RouteId::SessionsList, "limit=1&limit=2");
    let failure = reader.expect_err("a repeated key is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_malformed_path_parameter_is_rejected_before_the_handler_runs() {
    let request = RawRequest {
        route: RouteId::ApiKeyRevoke,
        // `match_route` binds any non-empty segment; the type is what refuses it.
        path: match_route(
            Plane::Central,
            HttpMethod::Delete,
            "/api/api-keys/not-an-id",
        )
        .expect("the template matches")
        .1,
        query: "",
        body: b"",
    };
    let failure = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeyRevoke),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("a malformed identifier is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_route_from_another_group_never_reaches_a_handler() {
    let request = raw(RouteId::SessionsList, "/api/sessions", "", b"");
    let failure = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::SessionsList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("a foreign route is an internal error");
    assert_eq!(failure.code, ErrorCode::InternalError);
}

#[test]
fn a_declared_failure_reaches_the_wire_unchanged() {
    assert!(route(RouteId::SessionsList).declares(ErrorCode::InvalidCursor));
    let request = raw(RouteId::SessionsList, "/api/sessions", "", b"");
    let failure = block_on(dispatch_sessions(
        &Stub::failing(ErrorCode::InvalidCursor),
        &context(RouteId::SessionsList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("the handler failed");
    assert_eq!(failure.code, ErrorCode::InvalidCursor);
}

#[test]
fn an_undeclared_failure_never_reaches_the_wire() {
    // `sessions_list` declares neither of these, so a handler that answers one
    // is a contract violation the boundary refuses rather than forwards.
    assert!(!route(RouteId::SessionsList).declares(ErrorCode::SessionNotIdle));
    let request = raw(RouteId::SessionsList, "/api/sessions", "", b"");
    let failure = block_on(dispatch_sessions(
        &Stub::failing(ErrorCode::SessionNotIdle),
        &context(RouteId::SessionsList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect_err("the handler failed");
    assert_eq!(failure.code, ErrorCode::InternalError);
    assert!(
        failure
            .message
            .as_deref()
            .is_some_and(|message| message.contains("sessions_list")),
        "the refusal does not name the operation: {:?}",
        failure.message
    );
}

// ---------------------------------------------------------------------------
// Replay identity
// ---------------------------------------------------------------------------

#[test]
fn a_route_that_requires_a_replay_key_refuses_a_request_without_one() {
    let request = raw(RouteId::ApiKeyCreate, "/api/api-keys", "", b"{}");
    let failure = request_identity(&context(RouteId::ApiKeyCreate), &request)
        .expect_err("a missing `Idempotency-Key` is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_route_that_requires_no_replay_key_refuses_one() {
    let mut cx = context(RouteId::SessionsList);
    cx.idempotency_key = Some(IdempotencyKey::parse("k").expect("key"));
    let request = raw(RouteId::SessionsList, "/api/sessions", "", b"");
    let failure =
        request_identity(&cx, &request).expect_err("an unused `Idempotency-Key` is a 400");
    assert_eq!(failure.code, ErrorCode::InvalidRequest);
}

#[test]
fn the_replay_intent_is_over_canonical_bytes_not_the_bytes_that_arrived() {
    let compact = br#"{"b":2,"a":1}"#;
    let spaced = br#"{ "a" : 1 , "b" : 2 }"#;
    let path = format!("/api/sessions/ses_{SUFFIX}/terminations");
    let first = canonical_intent(&raw(RouteId::SessionTerminate, &path, "", compact))
        .expect("canonical intent");
    let second =
        canonical_intent(&raw(RouteId::SessionTerminate, &path, "", spaced)).expect("intent");
    assert_eq!(first, second, "two spellings produced two intents");

    let mut cx = context(RouteId::SessionTerminate);
    cx.operation_id = Some(operation_id());
    let identity = request_identity(&cx, &raw(RouteId::SessionTerminate, &path, "", compact))
        .expect("operation identity");
    let RequestIdentity::Operation(identity) = identity else {
        panic!("an `Aex-Operation-Id` route produced another identity");
    };
    assert_eq!(identity.route, RouteId::SessionTerminate);
    assert_eq!(identity.intent, first);
}

#[test]
fn a_route_with_no_replay_identity_reports_none() {
    let request = raw(RouteId::SessionsList, "/api/sessions", "", b"");
    let identity = request_identity(&context(RouteId::SessionsList), &request).expect("identity");
    assert_eq!(identity, RequestIdentity::None);
}

// ---------------------------------------------------------------------------
// Parameter codecs
// ---------------------------------------------------------------------------

#[test]
fn every_parameter_type_round_trips_between_the_two_codecs() {
    assert_eq!(
        ApiKeyId::from_param(&api_key_id().to_param()).expect("id"),
        api_key_id()
    );
    assert_eq!(
        WorkspaceId::from_param(&workspace_id().to_param()).expect("id"),
        workspace_id()
    );
    for limit in LimitId::ALL {
        assert_eq!(
            LimitId::from_param(&limit.to_param()).expect("limit"),
            *limit
        );
    }
    for provider in ProviderId::ALL {
        assert_eq!(
            ProviderId::from_param(&provider.to_param()).expect("provider"),
            *provider
        );
    }
    for category in BillingUsageCategory::ALL {
        assert_eq!(
            BillingUsageCategory::from_param(&category.to_param()).expect("category"),
            *category
        );
    }
    for status in SessionStatus::ALL {
        assert_eq!(
            SessionStatus::from_param(&status.to_param()).expect("status"),
            *status
        );
    }
}

#[test]
fn a_wrong_kind_of_identifier_never_decodes_as_a_parameter() {
    let workspace = workspace_id().to_param();
    assert!(
        ApiKeyId::from_param(&workspace).is_err(),
        "a workspace id decoded as an API key id"
    );
}

#[test]
fn percent_coding_round_trips_and_refuses_a_truncated_escape() {
    for value in ["plain", "a b", "a/b?c=d&e", "ünïcøde", "100%"] {
        let encoded = percent_encode(value);
        assert!(!encoded.contains('/'), "`{encoded}` leaks a path separator");
        assert!(
            !encoded.contains('&'),
            "`{encoded}` leaks a field separator"
        );
        assert_eq!(percent_decode(&encoded).expect("decode"), value);
    }
    assert!(percent_decode("%A").is_err(), "a truncated escape decoded");
    assert!(percent_decode("%ZZ").is_err(), "a non-hex escape decoded");
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

#[test]
fn a_built_request_carries_the_declared_method_path_and_headers() {
    let request = api_key_revoke_request(api_key_id(), None).expect("request");
    assert_eq!(request.route, RouteId::ApiKeyRevoke);
    assert_eq!(request.method, HttpMethod::Delete);
    assert_eq!(request.path, format!("/api/api-keys/key_{SUFFIX}"));
    assert!(request.query.is_empty());
    assert!(request.body.is_none());
    assert_eq!(request.header(HEADER_ACCEPT), Some("application/json"));
    assert_eq!(request.header(HEADER_CONTENT_TYPE), None);
    assert_eq!(request.header(HEADER_IF_MATCH), None);

    let etag = ETag::parse("\"rev-7\"").expect("etag");
    let request = api_key_revoke_request(api_key_id(), Some(&etag)).expect("request");
    assert_eq!(request.header(HEADER_IF_MATCH), Some("\"rev-7\""));
}

#[test]
fn a_built_request_carries_the_identity_header_its_route_declares() {
    let request = session_terminate_request(
        session_id(),
        &serde_json::from_value(serde_json::json!({})).expect("body"),
        operation_id(),
    )
    .expect("request");
    assert_eq!(
        request.header(HEADER_OPERATION_ID),
        Some(format!("op_{SUFFIX}").as_str())
    );
    assert_eq!(request.header(HEADER_IDEMPOTENCY_KEY), None);
    assert_eq!(
        request.header(HEADER_CONTENT_TYPE),
        Some("application/json")
    );
}

#[test]
fn a_built_request_encodes_only_the_query_parameters_that_were_supplied() {
    let bare = sessions_list_request(&SessionsListQuery {
        cursor: None,
        limit: None,
        status: None,
    })
    .expect("request");
    assert!(
        bare.query.is_empty(),
        "an empty query rendered `{}`",
        bare.query
    );

    let filtered = sessions_list_request(&SessionsListQuery {
        cursor: None,
        limit: Some(25),
        status: Some(SessionStatus::Idle),
    })
    .expect("request");
    assert_eq!(
        filtered.query,
        "limit=25&status=idle",
        "the query is not in the registry's parameter order"
    );
}

#[test]
fn a_built_request_renders_a_required_query_parameter() {
    let request = api_keys_list_request(
        &serde_json::from_value(serde_json::json!({
            "workspaceId": format!("wsp_{SUFFIX}"),
        }))
        .expect("query"),
    )
    .expect("request");
    assert_eq!(request.query, format!("workspaceId=wsp_{SUFFIX}"));
}

#[test]
fn a_request_url_joins_the_origin_without_doubling_a_separator() {
    let base = BaseUrl::parse("https://api.aex.dev/").expect("base");
    let request = sessions_list_request(&SessionsListQuery {
        cursor: None,
        limit: Some(1),
        status: None,
    })
    .expect("request");
    assert_eq!(
        request.url(&base),
        "https://api.aex.dev/api/sessions?limit=1"
    );
}

#[test]
fn a_base_url_refuses_anything_that_is_not_a_bare_https_origin() {
    assert!(BaseUrl::parse("http://api.aex.dev").is_err());
    assert!(BaseUrl::parse("https://").is_err());
    assert!(BaseUrl::parse("https://api.aex.dev/v1").is_err());
    assert!(BaseUrl::parse("https://api.aex.dev?a=b").is_err());
    assert_eq!(
        BaseUrl::parse("https://api.aex.dev")
            .expect("base")
            .as_str(),
        "https://api.aex.dev"
    );
}

/// A transport that answers from a fixed script.
struct Scripted {
    /// The status every call answers with.
    status: u16,
    /// The body every call answers with.
    body: Vec<u8>,
}

impl Transport for Scripted {
    fn execute(
        &self,
        request: WireRequest,
    ) -> impl Future<Output = Result<WireResponse, TransportError>> + Send {
        let status = self.status;
        let body = self.body.clone();
        async move {
            assert!(request.path.starts_with("/api/"));
            Ok(WireResponse {
                status,
                etag: None,
                body,
            })
        }
    }
}

#[test]
fn a_client_call_decodes_the_declared_success_body() {
    let page = SessionListPage {
        items: vec![session_list_item()],
        next_cursor: None,
    };
    let client = WireClient::new(
        Scripted {
            status: 200,
            body: serde_json::to_vec(&page).expect("body"),
        },
        BaseUrl::parse("https://api.aex.dev").expect("base"),
    );
    let answer = block_on(client.sessions_list(&SessionsListQuery {
        cursor: None,
        limit: None,
        status: None,
    }))
    .expect("call");
    assert_eq!(answer, page);
}

#[test]
fn a_client_call_decodes_the_published_error_envelope() {
    let envelope = serde_json::json!({
        "error": {
            "code": "insufficient_scope",
            "message": "the credential lacks `sessions:read`",
            "requestId": "req-0002",
            "retryable": false,
        }
    });
    let client = WireClient::new(
        Scripted {
            status: 403,
            body: serde_json::to_vec(&envelope).expect("body"),
        },
        BaseUrl::parse("https://api.aex.dev").expect("base"),
    );
    let failure = block_on(client.sessions_list(&SessionsListQuery {
        cursor: None,
        limit: None,
        status: None,
    }))
    .expect_err("a 403 is not a success");
    assert_eq!(failure.code(), Some(ErrorCode::InsufficientScope));
    let ClientError::Api { status, route, .. } = failure else {
        panic!("a published envelope decoded as something else");
    };
    assert_eq!(status, 403);
    assert_eq!(route, RouteId::SessionsList);
}

#[test]
fn a_success_status_the_route_does_not_declare_is_never_a_success() {
    let client = WireClient::new(
        Scripted {
            status: 201,
            body: b"{}".to_vec(),
        },
        BaseUrl::parse("https://api.aex.dev").expect("base"),
    );
    let failure = block_on(client.sessions_list(&SessionsListQuery {
        cursor: None,
        limit: None,
        status: None,
    }))
    .expect_err("a 201 on a 200 route is not a success");
    assert!(matches!(failure, ClientError::Decode { .. }));
}

#[test]
fn a_path_writer_refuses_an_arity_that_does_not_match_the_template() {
    let writer = PathWriter::new(RouteId::SessionGet);
    let failure = writer.finish().expect_err("an unbound template is refused");
    assert!(matches!(failure, ClientError::Encode { .. }));

    let mut writer = PathWriter::new(RouteId::SessionGet);
    writer.bind(&session_id());
    assert_eq!(
        writer.finish().expect("path"),
        format!("/api/sessions/ses_{SUFFIX}")
    );
}

#[test]
fn a_query_writer_and_the_strict_reader_agree() {
    let mut writer = QueryWriter::new();
    writer.put("workspaceId", &workspace_id());
    writer.put_option("limit", Some(&7_u32));
    let query = writer.finish();
    let reader = QueryReader::parse(RouteId::ApiKeysList, &query).expect("reader");
    assert_eq!(
        reader.required::<WorkspaceId>("workspaceId").expect("id"),
        workspace_id()
    );
    assert_eq!(
        reader
            .optional_bounded::<u32>("limit", 1, 1000)
            .expect("limit"),
        Some(7)
    );
    assert_eq!(
        reader
            .optional::<aex_wire::Cursor>("cursor")
            .expect("cursor"),
        None
    );
}

#[test]
fn a_dispatch_outcome_of_a_group_without_a_stream_can_never_be_a_stream() {
    // `NoStream` is uninhabited, so this is a type-level assertion: the only
    // constructible arm for a non-streaming group is `Unary`.
    let query = format!("workspaceId=wsp_{SUFFIX}");
    let request = raw(RouteId::ApiKeysList, "/api/api-keys", &query, b"");
    let outcome: DispatchOutcome<aex_wire::dispatch::NoStream> = block_on(dispatch_api_keys(
        &Stub::ok(),
        &context(RouteId::ApiKeysList),
        request,
        RequestLimits::DEFAULT,
    ))
    .expect("dispatch");
    assert!(outcome.into_unary().is_some());
}

#[test]
fn regional_read_models_match_their_authoritative_producers() {
    let workspace = workspace_id().to_string();
    let attribution: aex_wire::models::UsageAttribution =
        serde_json::from_value(serde_json::json!({
            "region": "eu-west-1",
            "workspaceId": workspace,
            "source": "runtime_activity",
            "serviceTime": {
                "gte": "2026-08-01T00:00:00.000Z",
                "lt": "2026-08-01T01:00:00.000Z"
            }
        }))
        .expect("complete attribution without a regional rate book");
    assert_eq!(attribution.source, "runtime_activity");

    // `category` is the authority face, not a priced category: memory and
    // compute are one contiguous fact sequence behind one authority, so there
    // are three frontiers and not four.
    let frontier: aex_wire::models::UsageFrontier = serde_json::from_value(serde_json::json!({
        "region": "eu-west-1",
        "workspaceId": workspace_id().to_string(),
        "category": "compute",
        "acceptedSequence": "8",
        "projectedSequence": "7",
        "publishedSequence": "6",
        "settledSequence": "5",
        "completeThrough": "7",
        "includesThrough": "7",
        "state": "advancing"
    }))
    .expect("domain stage names and an as-yet unknown service-time frontier");
    assert_eq!(frontier.projected_sequence.to_string(), "7");
    assert_eq!(frontier.published_sequence.to_string(), "6");
    assert!(frontier.service_through.is_none());
    // A stalled fold names where it stopped and why. A lag is not a stall, and
    // a fold parked behind a poisoned record must not be indistinguishable
    // from one a few seconds behind.
    assert!(frontier.stalled_at.is_none());
    assert!(frontier.stall_reason.is_none());
    assert_eq!(frontier.category, aex_wire::models::UsageAuthority::Compute);

    // Memory has no frontier of its own to ask for.
    assert!(
        serde_json::from_str::<aex_wire::models::UsageAuthority>("\"memory\"").is_err(),
        "publishing a memory frontier would claim an independence that does \
         not exist"
    );

    // Session, run and operation are not groupable: every stored aggregate row
    // is keyed by a hash that includes the session, so grouping by one would
    // read hundreds of thousands of rows for one monthly total.
    for absent in ["session", "run", "operation"] {
        assert!(
            serde_json::from_str::<aex_wire::models::UsageGrouping>(&format!("\"{absent}\""))
                .is_err(),
            "`{absent}` must not be a publishable grouping axis"
        );
    }
}
