//! End-to-end control-plane coverage: a customer signs up, tops up, creates a key, runs a session,
//! sees the bill — over real HTTP against the control router, with a stub brain implementing
//! session/v1 from the contracts and the fake payments adapter. Every control-plane response
//! is validated against `contracts/control/v1/schemas.json`; the proxied session documents are
//! validated against Brain's owned session schema. Hosted customer flows are verified separately
//! during deployment.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aex_control::StorageLimits;
use aex_control::admission::{Admission, AdmissionConfig};
use aex_control::api::{AppState, internal_router, router};
use aex_control::brain::BrainClient;
use aex_control::customer_environment::CustomerEnvironmentGateway;
use aex_control::payments::{Checkout, FakePayments, PaymentStatus, Payments, RefundAttempt};
use aex_control::rating::RateCard;
use aex_control::store::{AccountRow, Db, KeyRow};
use aex_control::sweep::run_deletion_worker;
use aex_control::web::WebRuntime;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri, header};
use axum::response::Response;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const OPERATOR_TOKEN: &str = "aex_ad_A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2";
const EXECUTOR_TOKEN: &str = "brain-to-aex-private-test-token";
const GATEWAY_TOKEN: &str = "api-gateway-private-test-token-000000000000";

// ---- a stub brain: session/v1, just enough, contract-shaped ----

struct StubSession {
    doc: Value,
    events: Vec<Value>,
    seq: i64,
    turns: i64,
}

struct StubBrain {
    token: String,
    counter: AtomicI64,
    sessions: Mutex<HashMap<String, StubSession>>,
    create_requests: Mutex<HashMap<String, (String, String)>>,
    last_create_body: Mutex<Option<Value>>,
    ambiguous_create_responses: Mutex<HashSet<String>>,
    deletion_polls: Mutex<HashMap<String, usize>>,
    hidden_event_reads: Mutex<HashSet<String>>,
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| {
            format!(
                "id: {}\nevent: {}\ndata: {}\n\n",
                e["seq"],
                e["type"].as_str().unwrap_or("event"),
                e
            )
        })
        .collect()
}

fn stub_doc(id: &str, state: &str, storage: Value, turns: i64, last_seq: i64) -> Value {
    let now_ms = aex_control::now_ms();
    let now = aex_control::rfc3339(now_ms);
    let retain_until = aex_control::rfc3339(now_ms + 86_400_000);
    json!({
        "id": id,
        "root_id": id,
        "depth": 0,
        "object": "session",
        "state": state,
        "turn_state": "idle",
        "shape": "1gb",
        "model": {
            "component_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "world": "aex:model/model@1.0.0",
            "provider": "anthropic",
            "name": "stub-model",
            "context_window_tokens": 32768
        },
        "storage": storage,
        "created_at": now.clone(),
        "retain_until": retain_until,
        "updated_at": now,
        "turns": turns,
        "last_seq": last_seq,
        "metadata": {}
    })
}

async fn stub_handler(
    State(stub): State<Arc<StubBrain>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let path = uri.path().to_string();
    assert_eq!(
        auth,
        format!("Bearer {}", stub.token),
        "every private proxy route must authenticate to Brain with the operator token"
    );
    if path.starts_with("/v1/sessions") {
        assert!(
            headers
                .get("x-brain-tenant-id")
                .is_some_and(|value| !value.is_empty()),
            "every hosted session request must carry the authenticated tenant"
        );
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let respond = |status: u16, v: Value| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap()
    };
    let mut sessions = stub.sessions.lock().unwrap();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["internal", "v1", "customer-environment", "grants"]) => {
            assert!(headers.get("x-brain-tenant-id").is_some());
            respond(
                201,
                json!({
                    "url": "wss://customer-environment.example.test/connect",
                    "protocol": "aex.grant.test",
                    "expires_at": "2026-08-18T09:05:00Z",
                    "grant_id": "chg_test",
                    "observation_url": "https://api.example.test/v1/customer-environment/observations/chg_test",
                    "observation_token": "observation-grant"
                }),
            )
        }
        ("POST", ["internal", "v1", "customer-environment", "gateway"]) => {
            assert!(headers.get("x-brain-tenant-id").is_none());
            match headers["x-brain-route-key"].to_str().unwrap() {
                "$connect" => {
                    assert_eq!(headers["x-brain-connection-id"], "connection-1");
                    assert_eq!(headers["x-brain-request-id"], "request-1");
                    assert_eq!(headers["x-brain-source-ip"], "203.0.113.7");
                    assert_eq!(headers[header::SEC_WEBSOCKET_PROTOCOL], "aex.grant.test");
                    Response::builder()
                        .status(204)
                        .header(header::SEC_WEBSOCKET_PROTOCOL, "aex.grant.test")
                        .body(Body::empty())
                        .unwrap()
                }
                "$default" => {
                    assert_eq!(headers["x-brain-connection-id"], "connection-1");
                    assert_eq!(headers["x-brain-source-ip"], "203.0.113.7");
                    let frame: Value = serde_json::from_slice(&body).unwrap();
                    if frame["proof"] == "bound-frame-proof" {
                        Response::builder().status(204).body(Body::empty()).unwrap()
                    } else {
                        respond(
                            401,
                            json!({"error":{"code":"unauthorized", "message":"invalid frame proof"}}),
                        )
                    }
                }
                route => panic!("Aex forwarded untrusted customer-environment route {route}"),
            }
        }
        (
            "POST",
            [
                "internal",
                "v1",
                "customer-environment",
                "observations",
                grant_id,
            ],
        ) => {
            if *grant_id != "chg_test"
                || headers
                    .get("x-brain-observation-grant")
                    .and_then(|value| value.to_str().ok())
                    != Some("observation-grant")
            {
                return respond(
                    401,
                    json!({"error":{"code":"unauthorized", "message":"invalid observation grant"}}),
                );
            }
            assert_eq!(
                headers[header::AUTHORIZATION],
                format!("Bearer {}", stub.token)
            );
            assert_eq!(headers["x-brain-observation-grant"], "observation-grant");
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({
                    "type": "terminal",
                    "epoch": 7,
                    "operation_id": "op_test",
                    "request_digest": "a".repeat(64),
                    "ok": true,
                    "output": {"ok": true}
                })
            );
            Response::builder().status(204).body(Body::empty()).unwrap()
        }
        ("POST", ["v1", "sessions"]) => {
            let key = headers
                .get("Idempotency-Key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let request_hash = aex_control::identity::hash_secret(
                std::str::from_utf8(&body).expect("the control plane forwards JSON"),
            );
            if let Some(key) = &key
                && let Some((original_hash, id)) =
                    stub.create_requests.lock().unwrap().get(key).cloned()
            {
                if original_hash != request_hash {
                    return respond(
                        409,
                        json!({"error": {"code": "conflict", "message": "key reused"}}),
                    );
                }
                return respond(201, sessions.get(&id).unwrap().doc.clone());
            }
            let n = stub.counter.fetch_add(1, Ordering::SeqCst);
            let id = format!("ses_stub{n:020}");
            let create_body: Value = serde_json::from_slice(&body).expect("valid create body");
            *stub.last_create_body.lock().unwrap() = Some(create_body.clone());
            let mut doc = stub_doc(
                &id,
                "open",
                json!({"session_storage_bytes": 0, "upload_reserved_bytes": 0}),
                0,
                0,
            );
            if let Some(shape) = create_body["shape"].as_str() {
                doc["shape"] = Value::String(shape.to_owned());
            }
            sessions.insert(
                id.clone(),
                StubSession {
                    doc: doc.clone(),
                    events: Vec::new(),
                    seq: 0,
                    turns: 0,
                },
            );
            if let Some(key) = key {
                assert!(
                    key.starts_with("aex:acc_") && !key.contains("same-create-request"),
                    "the control plane must namespace and hash the customer key"
                );
                stub.create_requests
                    .lock()
                    .unwrap()
                    .insert(key.clone(), (request_hash, id));
                let ambiguous_once = create_body["metadata"]["test_ambiguous_create"]
                    .as_str()
                    .map(str::to_owned)
                    .as_deref()
                    == Some("once");
                if ambiguous_once && stub.ambiguous_create_responses.lock().unwrap().insert(key) {
                    return respond(
                        503,
                        json!({"error": {"code": "unavailable", "message": "lost create response"}}),
                    );
                }
            }
            respond(201, doc)
        }
        ("GET", ["v1", "session-changes"]) => {
            let mut data = sessions
                .values()
                .map(|session| {
                    json!({
                        "id": format!("change:{}:{}", session.doc["id"].as_str().unwrap(), session.seq),
                        "session": session.doc
                    })
                })
                .collect::<Vec<_>>();
            data.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
            respond(
                200,
                json!({
                    "object":"session.change.list",
                    "partition":0,
                    "partitions":1,
                    "watermark_ms":aex_control::now_ms(),
                    "data":data,
                    "has_more":false
                }),
            )
        }
        ("GET", ["v1", "sessions"]) => {
            let requested_state = uri.query().and_then(|query| {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("state="))
            });
            let mut data = sessions
                .values()
                // Deliberately model a lagging tenant GSI: native children are absent from
                // ordinary discovery even though the strongly consistent adjacency below has
                // already committed them.
                .filter(|session| session.doc["depth"] == 0)
                .filter(|session| requested_state.is_none_or(|state| session.doc["state"] == state))
                .map(|session| session.doc.clone())
                .collect::<Vec<_>>();
            data.sort_by(|left, right| {
                right["updated_at"]
                    .as_str()
                    .cmp(&left["updated_at"].as_str())
            });
            respond(
                200,
                json!({
                    "object": "list",
                    "data": data,
                    "has_more": false
                }),
            )
        }
        ("GET", ["v1", "sessions", id]) => match sessions.get(*id) {
            Some(s) => respond(200, s.doc.clone()),
            None => respond(
                404,
                json!({"error": {"code": "not_found", "message": "no such session"}}),
            ),
        },
        ("POST", ["v1", "sessions", id, "messages"]) => {
            let spawn_native_child = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| value["content"].as_str().map(str::to_owned))
                .as_deref()
                == Some("spawn native child");
            let (turn_id, first) = {
                let Some(s) = sessions.get_mut(*id) else {
                    return respond(
                        404,
                        json!({"error": {"code": "not_found", "message": "no such session"}}),
                    );
                };
                // A turn that took exactly 1.5 s, journaled in the past: replay-deterministic.
                let t0 = aex_control::now_ms() - 10_000;
                let turn_id = format!("trn_stub{:020}", s.turns);
                let mut ev = |ty: &str, at: i64, extra: Value| {
                    s.seq += 1;
                    let mut e = json!({
                        "type": ty, "seq": s.seq, "at": aex_control::rfc3339(at),
                        "session_id": id, "turn_id": turn_id,
                    });
                    if let (Some(o), Some(x)) = (e.as_object_mut(), extra.as_object()) {
                        o.extend(x.clone());
                    }
                    s.events.push(e.clone());
                    e
                };
                let first = ev("turn.started", t0, json!({}));
                ev(
                    "tool.call",
                    t0 + 500,
                    json!({"agent_id":"root", "call_id":"call_search", "name":"web_search",
                           "input":{"query":"aex"}, "detach":false}),
                );
                ev(
                    "tool.result",
                    t0 + 1_000,
                    json!({"agent_id":"root", "call_id":"call_search", "name":"web_search",
                           "outcome":"completed", "duration_ms":500,
                           "output_preview":"{\"results\":[]}", "truncated":false}),
                );
                // Aex's protocol Tool can appear in Brain's operator journal, but its raw candidate
                // must never cross the hosted public event stream.
                ev(
                    "tool.call",
                    t0 + 1_100,
                    json!({"agent_id":"root", "call_id":"call_output", "name":"aex_submit_output",
                           "input":{"private_candidate":"must-not-leak"}, "detach":false}),
                );
                ev(
                    "tool.result",
                    t0 + 1_200,
                    json!({"agent_id":"root", "call_id":"call_output", "name":"aex_submit_output",
                           "outcome":"completed", "duration_ms":100,
                           "output_preview":"Structured output accepted.", "truncated":false}),
                );
                ev(
                    "assistant.message",
                    t0 + 1_400,
                    json!({"agent_id": "root", "text": "done"}),
                );
                ev(
                    "turn.completed",
                    t0 + 1_500,
                    json!({"stop_reason": "end_turn", "rounds": 1, "tool_calls": 2}),
                );
                s.turns += 1;
                s.doc = stub_doc(
                    id,
                    "open",
                    json!({"session_storage_bytes": 0, "upload_reserved_bytes": 0}),
                    s.turns,
                    s.seq,
                );
                (turn_id, first)
            };
            if spawn_native_child {
                let child_id = format!(
                    "ses_hidden{:018}",
                    stub.counter.fetch_add(1, Ordering::SeqCst)
                );
                let child_t0 = aex_control::now_ms() - 3_600_000;
                let child_turn = "trn_hidden000000000000000001";
                let events = vec![
                    json!({"type":"turn.started", "seq":1, "at":aex_control::rfc3339(child_t0),
                           "session_id":child_id, "turn_id":child_turn}),
                    json!({"type":"turn.completed", "seq":2,
                           "at":aex_control::rfc3339(child_t0 + 3_600),
                           "session_id":child_id, "turn_id":child_turn,
                           "stop_reason":"end_turn", "rounds":1, "tool_calls":0}),
                    json!({"type":"storage.usage", "seq":3,
                           "at":aex_control::rfc3339(child_t0 + 10_000),
                           "session_id":child_id,
                           "storage":{"session_storage_bytes":0,
                                      "upload_reserved_bytes":500_000_000}}),
                    json!({"type":"storage.usage", "seq":4,
                           "at":aex_control::rfc3339(child_t0 + 11_000),
                           "session_id":child_id,
                           "storage":{"session_storage_bytes":500_000_000,
                                      "upload_reserved_bytes":0}}),
                    json!({"type":"storage.usage", "seq":5,
                           "at":aex_control::rfc3339(child_t0 + 1_811_000),
                           "session_id":child_id,
                           "storage":{"session_storage_bytes":0,
                                      "upload_reserved_bytes":0}}),
                ];
                let mut child_doc = stub_doc(
                    &child_id,
                    "ended",
                    json!({"session_storage_bytes":0, "upload_reserved_bytes":0}),
                    1,
                    5,
                );
                child_doc["root_id"] = json!(id);
                child_doc["parent_id"] = json!(id);
                child_doc["depth"] = json!(1);
                child_doc["created_at"] = json!(aex_control::rfc3339(child_t0));
                child_doc["updated_at"] = json!(aex_control::rfc3339(child_t0 + 1_811_000));
                sessions.insert(
                    child_id,
                    StubSession {
                        doc: child_doc,
                        events,
                        seq: 5,
                        turns: 1,
                    },
                );
            }
            respond(
                202,
                json!({"session_id": id, "turn_id": turn_id, "seq": first["seq"]}),
            )
        }
        ("GET", ["v1", "sessions", id, "events"]) => {
            let Some(s) = sessions.get(*id) else {
                return respond(
                    404,
                    json!({"error": {"code": "not_found", "message": "no such session"}}),
                );
            };
            if s.doc["depth"].as_u64().unwrap_or_default() > 0 {
                stub.hidden_event_reads
                    .lock()
                    .unwrap()
                    .insert((*id).to_owned());
            }
            let after: i64 = uri
                .query()
                .and_then(|q| {
                    q.split('&')
                        .find_map(|kv| kv.strip_prefix("after="))
                        .and_then(|v| v.parse().ok())
                })
                .unwrap_or(0);
            let through: i64 = uri
                .query()
                .and_then(|q| {
                    q.split('&')
                        .find_map(|kv| kv.strip_prefix("through="))
                        .and_then(|v| v.parse().ok())
                })
                .unwrap_or(s.seq);
            let visible: Vec<Value> = s
                .events
                .iter()
                .filter(|e| {
                    let seq = e["seq"].as_i64().unwrap_or(0);
                    seq > after && seq <= through
                })
                .cloned()
                .collect();
            let mut body = sse(&visible);
            body.push_str(&format!(
                "event: replay.complete\ndata: {}\n\n",
                json!({"type":"replay.complete","session_id":id,"through_seq":through})
            ));
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }
        ("POST", ["v1", "sessions", id, "end"]) => {
            if !sessions.contains_key(*id) {
                return respond(
                    404,
                    json!({"error": {"code": "not_found", "message": "no such session"}}),
                );
            }
            let mut accepted = sessions.get(*id).unwrap().doc.clone();
            accepted["state"] = json!("ending");
            for (session_id, session) in sessions.iter_mut() {
                if session_id == *id || session.doc["root_id"].as_str() == Some(*id) {
                    session.doc["state"] = json!("ended");
                }
            }
            respond(202, accepted)
        }
        ("GET", ["v1", "sessions", id, "children"]) => {
            let mut data = sessions
                .values()
                .filter(|session| session.doc["parent_id"].as_str() == Some(*id))
                .map(|session| session.doc.clone())
                .collect::<Vec<_>>();
            data.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
            respond(200, json!({"object":"list", "data":data, "has_more":false}))
        }
        ("DELETE", ["v1", "sessions", id]) => {
            let unsettled_descendant = {
                let settled = stub.hidden_event_reads.lock().unwrap();
                sessions.iter().any(|(session_id, session)| {
                    session.doc["root_id"].as_str() == Some(*id)
                        && session.doc["depth"].as_u64().unwrap_or_default() > 0
                        && !settled.contains(session_id)
                })
            };
            if unsettled_descendant {
                return respond(
                    409,
                    json!({"error":{"code":"conflict", "message":"unsettled hidden child"}}),
                );
            }
            let removed: Vec<_> = sessions
                .iter()
                .filter(|(session_id, session)| {
                    session_id.as_str() == *id || session.doc["root_id"].as_str() == Some(*id)
                })
                .map(|(session_id, _)| session_id.clone())
                .collect();
            for session_id in &removed {
                sessions.remove(session_id);
            }
            if !removed.is_empty() {
                stub.deletion_polls
                    .lock()
                    .unwrap()
                    .entry((*id).to_owned())
                    .or_insert(0);
                Response::builder()
                    .status(202)
                    .header(header::LOCATION, format!("/v1/sessions/{id}/deletion"))
                    .header(header::RETRY_AFTER, "1")
                    .body(Body::empty())
                    .unwrap()
            } else {
                respond(
                    404,
                    json!({"error": {"code": "not_found", "message": "no such session"}}),
                )
            }
        }
        ("GET", ["v1", "sessions", id, "deletion"]) => {
            let mut deletions = stub.deletion_polls.lock().unwrap();
            let Some(polls) = deletions.get_mut(*id) else {
                return respond(
                    404,
                    json!({"error":{"code":"not_found", "message":"no deletion"}}),
                );
            };
            *polls += 1;
            let succeeded = *polls >= 2;
            let mut response = respond(
                200,
                json!({
                    "object":"session.deletion",
                    "session_id":id,
                    "state":if succeeded { "succeeded" } else { "deleting" },
                    "requested_at_ms":1,
                    "updated_at_ms":*polls as i64 + 1,
                    "completed_at_ms":if succeeded { Some(aex_control::now_ms()) } else { None }
                }),
            );
            if !succeeded {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, "1".parse().unwrap());
            }
            response
        }
        _ => respond(
            404,
            json!({"error": {"code": "not_found", "message": format!("stub: {method} {path}")}}),
        ),
    }
}

async fn spawn_stub_brain(token: &str) -> String {
    spawn_stub_brain_with_state(token).await.0
}

async fn spawn_stub_brain_with_state(token: &str) -> (String, Arc<StubBrain>) {
    let stub = Arc::new(StubBrain {
        token: token.to_string(),
        counter: AtomicI64::new(0),
        sessions: Mutex::new(HashMap::new()),
        create_requests: Mutex::new(HashMap::new()),
        last_create_body: Mutex::new(None),
        ambiguous_create_responses: Mutex::new(HashSet::new()),
        deletion_polls: Mutex::new(HashMap::new()),
        hidden_event_reads: Mutex::new(HashSet::new()),
    });
    let app = axum::Router::new()
        .fallback(stub_handler)
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, stub)
}

async fn spawn_control(brain_url: &str, brain_token: &str, limits: (i64, i64)) -> String {
    spawn_control_with_payments(brain_url, brain_token, limits, Arc::new(FakePayments)).await
}

async fn spawn_control_with_payments(
    brain_url: &str,
    brain_token: &str,
    limits: (i64, i64),
    payments: Arc<dyn Payments>,
) -> String {
    spawn_control_with_product_limits(brain_url, brain_token, limits, 100, payments).await
}

async fn spawn_control_with_product_limits(
    brain_url: &str,
    brain_token: &str,
    limits: (i64, i64),
    max_retained_root_sessions: i64,
    payments: Arc<dyn Payments>,
) -> String {
    let db = Db::open_memory().unwrap();
    let brain = BrainClient::new(brain_url, brain_token);
    let card = RateCard::default();
    let state = AppState {
        db: db.clone(),
        brain: brain.clone(),
        payments,
        stripe_webhook: None,
        card: card.clone(),
        operator_token_hash: Some(aex_control::identity::hash_secret(OPERATOR_TOKEN)),
        external_executor_token_hash: None,
        customer_environment_gateway: Some(
            CustomerEnvironmentGateway::new(
                GATEWAY_TOKEN,
                vec!["127.0.0.0/8".parse().unwrap()],
                vec!["198.51.100.0/24".parse().unwrap()],
            )
            .unwrap(),
        ),
        web: WebRuntime::hosted(None),
        admission: Admission::new(AdmissionConfig::default()).unwrap(),
        create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
        inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
        default_limits: limits,
        max_retained_root_sessions,
        storage_limits: StorageLimits::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(run_deletion_worker(db, brain, card));
    tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    base
}

async fn spawn_control_without_deletion_worker(
    brain_url: &str,
    brain_token: &str,
    db: Db,
) -> String {
    let state = AppState {
        db,
        brain: BrainClient::new(brain_url, brain_token),
        payments: Arc::new(FakePayments),
        stripe_webhook: None,
        card: RateCard::default(),
        operator_token_hash: Some(aex_control::identity::hash_secret(OPERATOR_TOKEN)),
        external_executor_token_hash: None,
        customer_environment_gateway: None,
        web: WebRuntime::hosted(None),
        admission: Admission::new(AdmissionConfig::default()).unwrap(),
        create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
        inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
        default_limits: (10, 30),
        max_retained_root_sessions: 100,
        storage_limits: StorageLimits::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    base
}

async fn spawn_control_state(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    base
}

async fn spawn_internal_executor(brain_url: &str, brain_token: &str) -> String {
    let state = AppState {
        db: Db::open_memory().unwrap(),
        brain: BrainClient::new(brain_url, brain_token),
        payments: Arc::new(FakePayments),
        stripe_webhook: None,
        card: RateCard::default(),
        operator_token_hash: None,
        external_executor_token_hash: Some(aex_control::identity::hash_secret(EXECUTOR_TOKEN)),
        customer_environment_gateway: None,
        web: WebRuntime::hosted(None),
        admission: Admission::new(AdmissionConfig::default()).unwrap(),
        create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
        inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
        default_limits: (10, 30),
        max_retained_root_sessions: 100,
        storage_limits: StorageLimits::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, internal_router(state)).await.unwrap() });
    base
}

struct FlakyRefundPayments {
    refund_attempts: AtomicUsize,
}

struct ReturnPagePayments {
    checks: AtomicUsize,
}

#[async_trait::async_trait]
impl Payments for ReturnPagePayments {
    fn name(&self) -> &'static str {
        "stripe"
    }

    async fn create_checkout(
        &self,
        topup_id: &str,
        _amount_cents: i64,
    ) -> aex_control::Result<Checkout> {
        Ok(Checkout {
            provider_ref: format!("cs_test_{topup_id}"),
            url: "https://checkout.stripe.test/session".into(),
        })
    }

    async fn check(&self, provider_ref: &str) -> aex_control::Result<PaymentStatus> {
        assert!(provider_ref.starts_with("cs_test_top_"));
        self.checks.fetch_add(1, Ordering::SeqCst);
        Ok(PaymentStatus::Paid)
    }

    async fn refund(
        &self,
        _topup_id: &str,
        _provider_ref: &str,
        refund_id: &str,
        _amount_cents: i64,
    ) -> aex_control::Result<RefundAttempt> {
        Ok(RefundAttempt::Succeeded {
            provider_ref: format!("re_{refund_id}"),
        })
    }
}

#[async_trait::async_trait]
impl Payments for FlakyRefundPayments {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn create_checkout(
        &self,
        topup_id: &str,
        _amount_cents: i64,
    ) -> aex_control::Result<Checkout> {
        Ok(Checkout {
            provider_ref: format!("fake_{topup_id}"),
            url: format!("https://payments.invalid/checkout/{topup_id}"),
        })
    }

    async fn check(&self, _provider_ref: &str) -> aex_control::Result<PaymentStatus> {
        Ok(PaymentStatus::Paid)
    }

    async fn refund(
        &self,
        _topup_id: &str,
        _provider_ref: &str,
        refund_id: &str,
        _amount_cents: i64,
    ) -> aex_control::Result<RefundAttempt> {
        if self.refund_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(aex_control::Error::Payment(
                "simulated uncertain response".into(),
            ));
        }
        Ok(RefundAttempt::Succeeded {
            provider_ref: format!("fake_{refund_id}"),
        })
    }
}

// ---- schema validation of every wire response ----

fn validator(schema_json: &str, type_name: &str) -> jsonschema::Validator {
    let mut schema: Value = serde_json::from_str(schema_json).unwrap();
    let obj = schema.as_object_mut().unwrap();
    obj.insert("$ref".into(), json!(format!("#/$defs/{type_name}")));
    obj.remove("$id");
    jsonschema::draft202012::new(&schema).unwrap()
}

fn assert_valid(schema_json: &str, type_name: &str, value: &Value) {
    let v = validator(schema_json, type_name);
    let errors: Vec<String> = v.iter_errors(value).map(|e| format!("{e}")).collect();
    assert!(
        errors.is_empty(),
        "{type_name} violation on {value}:\n  {}",
        errors.join("\n  ")
    );
}

fn control_valid(type_name: &str, value: &Value) {
    assert_valid(aex_contracts::CONTROL_SCHEMA_JSON, type_name, value);
}

async fn json_of(resp: reqwest::Response, expect: u16) -> Value {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    assert_eq!(status, expect, "body: {text}");
    serde_json::from_str(&text).unwrap()
}

async fn invited_signup(http: &reqwest::Client, base: &str, email: &str) -> Value {
    let submission = json_of(
        http.post(format!("{base}/v1/waitlist"))
            .json(&json!({"email": email}))
            .send()
            .await
            .unwrap(),
        202,
    )
    .await;
    control_valid("WaitlistSubmission", &submission);

    let invitation = json_of(
        http.post(format!("{base}/v1/admin/invitations"))
            .bearer_auth(OPERATOR_TOKEN)
            .json(&json!({"email": email}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("InvitationCreated", &invitation);

    let created = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": email, "invite_token": invitation["invite_token"]}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("AccountCreated", &created);
    created
}

// ---- the gate flow ----

#[tokio::test]
async fn private_executor_requires_its_service_credential_and_routes_pinned_capabilities() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_internal_executor(&brain_url, brain_token).await;
    let http = reqwest::Client::new();
    let body = json!({
        "session_id": "ses_01HZZZZZZZZZZZZZZZZZZZZZZZ",
        "turn_id": "trn_01HZZZZZZZZZZZZZZZZZZZZZZZ",
        "agent_id": "root",
        "call_id": "call_01HZZZZZZZZZZZZZZZZZZZZZZZ",
        "name": "web_fetch",
        "input": {"url": "https://169.254.169.254/latest/meta-data/"},
        "context": {"brain.capability": "aex.web.fetch"}
    });

    let missing = http
        .post(format!("{base}/internal/v1/tools/call"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 401);
    let wrong = http
        .post(format!("{base}/internal/v1/tools/call"))
        .bearer_auth("wrong-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401);
    let accepted = json_of(
        http.post(format!("{base}/internal/v1/tools/call"))
            .bearer_auth(EXECUTOR_TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(accepted["outcome"], "failed");
    assert_eq!(accepted["disposition"], "continue");
    assert!(accepted["content"].as_str().unwrap().contains("SSRF guard"));
    let replay = json_of(
        http.post(format!("{base}/internal/v1/tools/call"))
            .bearer_auth(EXECUTOR_TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(replay, accepted);

    // A valid host request can carry message metadata well beyond the obsolete 128 KiB extractor
    // ceiling. The private hop accepts it through Brain's one shared 512 KiB wire bound.
    let mut large = body.clone();
    large["call_id"] = json!("call_01HZZZZZZZZZZZZZZZZZZZZZZY");
    large["context"]["message.metadata"] = json!("x".repeat(256 * 1024));
    let large_response = json_of(
        http.post(format!("{base}/internal/v1/tools/call"))
            .bearer_auth(EXECUTOR_TOKEN)
            .json(&large)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(large_response["outcome"], "failed");

    let mut reused = body;
    reused["input"]["url"] = "https://127.0.0.1/".into();
    let mismatch = http
        .post(format!("{base}/internal/v1/tools/call"))
        .bearer_auth(EXECUTOR_TOKEN)
        .json(&reused)
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status().as_u16(), 409);
}

#[tokio::test(flavor = "multi_thread")]
async fn topup_create_replays_on_the_idempotency_key() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();
    let created = invited_signup(&http, &base, "topup-idem@example.com").await;
    let at = created["account_token"].as_str().unwrap().to_string();

    // The header is required: money creation without a replay identity is refused.
    let missing = http
        .post(format!("{base}/v1/topups"))
        .bearer_auth(&at)
        .json(&json!({"amount_cents": 1000}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing.status().as_u16(),
        400,
        "Idempotency-Key is required"
    );

    let first = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(&at)
            .header("Idempotency-Key", "retry-after-lost-response")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    // The lost-response retry: same key replays the same topup and checkout link — no
    // second live payment is minted.
    let replay = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(&at)
            .header("Idempotency-Key", "retry-after-lost-response")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(first["id"], replay["id"]);
    assert_eq!(first["checkout_url"], replay["checkout_url"]);

    // The same key with a different amount is a conflict, never a silent second charge.
    let conflicted = http
        .post(format!("{base}/v1/topups"))
        .bearer_auth(&at)
        .header("Idempotency-Key", "retry-after-lost-response")
        .json(&json!({"amount_cents": 2000}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflicted.status().as_u16(), 409);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_signup_response_heals_by_retrying_the_same_invite() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();
    let email = "lost-response@example.com";
    json_of(
        http.post(format!("{base}/v1/waitlist"))
            .json(&json!({"email": email}))
            .send()
            .await
            .unwrap(),
        202,
    )
    .await;
    let invitation = json_of(
        http.post(format!("{base}/v1/admin/invitations"))
            .bearer_auth(OPERATOR_TOKEN)
            .json(&json!({"email": email}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let invite_token = invitation["invite_token"].as_str().unwrap();
    let created = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": email, "invite_token": invite_token}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let lost_token = created["account_token"].as_str().unwrap();

    // The 201 was lost: the retry with the same one-time invite rotates the token instead
    // of dead-ending on Conflict. The invite is the proof of authority.
    let retried = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": email, "invite_token": invite_token}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(retried["account"]["id"], created["account"]["id"]);
    let fresh_token = retried["account_token"].as_str().unwrap();
    assert_ne!(fresh_token, lost_token);

    // The rotated-away token is dead; the fresh one authenticates.
    let dead = http
        .get(format!("{base}/v1/account"))
        .bearer_auth(lost_token)
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status().as_u16(), 401);
    let alive = http
        .get(format!("{base}/v1/account"))
        .bearer_auth(fresh_token)
        .send()
        .await
        .unwrap();
    assert_eq!(alive.status().as_u16(), 200);

    // A wrong invite for a joined email still refuses.
    let stranger = http
        .post(format!("{base}/v1/accounts"))
        .json(&json!({
            "email": email,
            "invite_token": "aex_iv_A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status().as_u16(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operator_reinvites_a_joined_but_never_used_account() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();
    let email = "lost-forever@example.com";
    let created = invited_signup(&http, &base, email).await;

    // The account token is gone and the retry window has passed: the operator re-invite
    // deletes the never-used account and reopens the invite.
    let reinvited = json_of(
        http.post(format!("{base}/v1/admin/invitations"))
            .bearer_auth(OPERATOR_TOKEN)
            .json(&json!({"email": email}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let rejoined = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": email, "invite_token": reinvited["invite_token"]}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_ne!(rejoined["account"]["id"], created["account"]["id"]);

    // An account that has been used (holds an api key) refuses the escape hatch.
    let at = rejoined["account_token"].as_str().unwrap();
    json_of(
        http.post(format!("{base}/v1/keys"))
            .bearer_auth(at)
            .json(&json!({"name": "in-use"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let refused = http
        .post(format!("{base}/v1/admin/invitations"))
        .bearer_auth(OPERATOR_TOKEN)
        .json(&json!({"email": email}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 409, "in-use accounts stay put");
}

#[tokio::test(flavor = "multi_thread")]
async fn checkout_return_reconciles_by_session_id_without_disclosing_the_topup() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let payments = Arc::new(ReturnPagePayments {
        checks: AtomicUsize::new(0),
    });
    let base =
        spawn_control_with_payments(&brain_url, brain_token, (10, 30), payments.clone()).await;
    let http = reqwest::Client::new();
    let created = invited_signup(&http, &base, "checkout-return@example.com").await;
    let account_token = created["account_token"].as_str().unwrap();
    let topup = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(account_token)
            .header("Idempotency-Key", "e2e-topup-key-01")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let checkout_session_id = format!("cs_test_{}", topup["id"].as_str().unwrap());

    let returned = http
        .get(format!("{base}/v1/topups/checkout/{checkout_session_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(returned.status().as_u16(), 200);
    assert_eq!(returned.bytes().await.unwrap().len(), 0);
    assert_eq!(payments.checks.load(Ordering::SeqCst), 1);

    let balance = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(balance["microusd"], "10000000");

    let replay = http
        .get(format!("{base}/v1/topups/checkout/{checkout_session_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status().as_u16(), 200);
    assert_eq!(payments.checks.load(Ordering::SeqCst), 1);

    let unknown = http
        .get(format!("{base}/v1/topups/checkout/cs_test_unknown"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status().as_u16(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn root_delete_then_hidden_deep_child_delete_hydrates_one_atomic_overlap_chain() {
    let brain_token = "operator-token";
    let (brain_url, stub) = spawn_stub_brain_with_state(brain_token).await;
    let now = aex_control::now_ms();
    let descendant = |id: &str, root: &str, parent: &str, depth: i64| {
        let mut document = stub_doc(
            id,
            "ended",
            json!({"session_storage_bytes":0, "upload_reserved_bytes":0}),
            0,
            0,
        );
        document["root_id"] = json!(root);
        document["parent_id"] = json!(parent);
        document["depth"] = json!(depth);
        document["created_at"] = json!(aex_control::rfc3339(now - 1_000 + depth));
        document
    };
    let root_id = "ses_root000000000000000001";
    let child_id = "ses_child00000000000000001";
    let grandchild_id = "ses_grand00000000000000001";
    let bad_parent_id = "ses_badparent000000000001";
    let bad_child_id = "ses_badchild0000000000001";
    {
        let mut sessions = stub.sessions.lock().unwrap();
        sessions.insert(
            root_id.into(),
            StubSession {
                doc: stub_doc(
                    root_id,
                    "ended",
                    json!({"session_storage_bytes":0, "upload_reserved_bytes":0}),
                    0,
                    0,
                ),
                events: Vec::new(),
                seq: 0,
                turns: 0,
            },
        );
        for (id, document) in [
            (child_id, descendant(child_id, root_id, root_id, 1)),
            (
                grandchild_id,
                descendant(grandchild_id, root_id, child_id, 2),
            ),
            (
                bad_parent_id,
                descendant(bad_parent_id, root_id, root_id, 2),
            ),
            (
                bad_child_id,
                descendant(bad_child_id, root_id, bad_parent_id, 2),
            ),
        ] {
            sessions.insert(
                id.into(),
                StubSession {
                    doc: document,
                    events: Vec::new(),
                    seq: 0,
                    turns: 0,
                },
            );
        }
    }

    let db = Db::open_memory().unwrap();
    let account_id = "acc_hidden_chain";
    let secret = "aex_sk_hidden_chain_test";
    db.create_account(
        AccountRow {
            id: account_id.into(),
            email: "hidden-chain@example.test".into(),
            created_ms: now,
            max_concurrent_sessions: 10,
            session_creates_per_hour: 30,
        },
        "hidden-chain@example.test".into(),
        aex_control::identity::hash_secret("account-token"),
    )
    .await
    .unwrap();
    db.create_key(
        KeyRow {
            id: "key_hidden_chain".into(),
            account_id: account_id.into(),
            name: "hidden-chain".into(),
            prefix: "aex_sk_hidden".into(),
            created_ms: now,
            last_used_ms: None,
            revoked_ms: None,
        },
        aex_control::identity::hash_secret(secret),
    )
    .await
    .unwrap();
    let base = spawn_control_without_deletion_worker(&brain_url, brain_token, db.clone()).await;
    let http = reqwest::Client::new();

    assert_eq!(
        http.delete(format!("{base}/v1/sessions/{root_id}"))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        202
    );
    assert!(db.session(child_id.into()).await.unwrap().is_none());
    assert_eq!(
        http.delete(format!("{base}/v1/sessions/{grandchild_id}"))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        202
    );
    assert_eq!(
        db.deletion(grandchild_id.into())
            .await
            .unwrap()
            .unwrap()
            .anchor_id,
        root_id
    );
    assert_eq!(db.session(child_id.into()).await.unwrap().unwrap().depth, 1);

    // A contradictory strong parent projection fails before either that parent or a deletion job
    // can be committed. The selected HEAD may remain as an owned discovery hint.
    let bad = http
        .delete(format!("{base}/v1/sessions/{bad_child_id}"))
        .bearer_auth(secret)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status().as_u16(), 502);
    assert!(db.deletion(bad_child_id.into()).await.unwrap().is_none());
    assert!(db.session(bad_parent_id.into()).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_create_recovery_keeps_its_original_admission_at_zero_balance() {
    let brain_token = "operator-token";
    let (brain_url, stub) = spawn_stub_brain_with_state(brain_token).await;
    let db = Db::open_memory().unwrap();
    let account_id = "acc_ambiguous_create";
    let api_key = "aex_sk_ambiguous_create_test";
    db.create_account(
        AccountRow {
            id: account_id.into(),
            email: "ambiguous-create@example.test".into(),
            created_ms: 1,
            max_concurrent_sessions: 10,
            session_creates_per_hour: 30,
        },
        "ambiguous-create@example.test".into(),
        aex_control::identity::hash_secret("unused-account-token"),
    )
    .await
    .unwrap();
    db.create_key(
        KeyRow {
            id: "key_ambiguous_create".into(),
            account_id: account_id.into(),
            name: "ambiguous-create".into(),
            prefix: "aex_sk_ambiguou".into(),
            created_ms: 1,
            last_used_ms: None,
            revoked_ms: None,
        },
        aex_control::identity::hash_secret(api_key),
    )
    .await
    .unwrap();

    let admission = Admission::new(AdmissionConfig::default()).unwrap();
    // The first dispatch was admitted while credit was positive. Keep the durable ledger at zero
    // so invalidating this warm value precisely models balance being spent before response
    // recovery, without inventing a test-only ledger mutation API.
    let initial = admission.begin_reconciliation(account_id).unwrap();
    assert!(admission.mark_reconciled(account_id, 1, aex_control::now_ms(), initial.generation(),));
    drop(initial);
    let state = AppState {
        db: db.clone(),
        brain: BrainClient::new(&brain_url, brain_token),
        payments: Arc::new(FakePayments),
        stripe_webhook: None,
        card: RateCard::default(),
        operator_token_hash: None,
        external_executor_token_hash: None,
        customer_environment_gateway: None,
        web: WebRuntime::hosted(None),
        admission: admission.clone(),
        create_body_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        message_body_slots: Arc::new(tokio::sync::Semaphore::new(256)),
        inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(64)),
        default_limits: (10, 30),
        max_retained_root_sessions: 100,
        storage_limits: StorageLimits::default(),
    };
    let base = spawn_control_state(state).await;
    let http = reqwest::Client::new();
    let request = json!({
        "model": {
            "provider": "openai",
            "name": "stub-model",
            "api_key": "provider-secret"
        },
        "metadata": {"test_ambiguous_create": "once"}
    });

    let first = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(api_key)
        .header("Idempotency-Key", "recover-create-after-balance")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 503);
    assert_eq!(db.balance(account_id.into()).await.unwrap(), 0);
    admission.invalidate_balance(account_id);

    let recovered = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(api_key)
        .header("Idempotency-Key", "recover-create-after-balance")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status().as_u16(), 201);
    assert_eq!(
        stub.counter.load(Ordering::SeqCst),
        1,
        "recovery must resolve the original Brain identity, not create another root",
    );
    assert_eq!(db.balance(account_id.into()).await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_signs_up_tops_up_keys_runs_and_sees_the_bill() {
    let brain_token = "operator-token";
    let (brain_url, stub_brain) = spawn_stub_brain_with_state(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();

    let internal_on_public_port = http
        .post(format!("{base}/internal/v1/tools/call"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        internal_on_public_port.status().as_u16(),
        404,
        "the Brain executor must not be mounted on the public listener"
    );

    // Join the canonical waitlist, receive a one-time invitation, and sign up. The account
    // token appears once; the dashboard can establish its HttpOnly session now.
    let created = invited_signup(&http, &base, "stranger@example.com").await;
    let at = created["account_token"].as_str().unwrap().to_string();
    let account_id = created["account"]["id"].as_str().unwrap().to_string();

    let hidden = http
        .get(format!("{base}/v1/admin/waitlist"))
        .send()
        .await
        .unwrap();
    assert_eq!(hidden.status().as_u16(), 401);
    let waitlist = json_of(
        http.get(format!("{base}/v1/admin/waitlist"))
            .bearer_auth(OPERATOR_TOKEN)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("WaitlistEntryList", &waitlist);
    assert_eq!(waitlist["data"][0]["status"], "joined");

    // The invite was consumed, so a second signup is forbidden. A garbage email fails fast.
    let dup = http
        .post(format!("{base}/v1/accounts"))
        .json(&json!({
            "email": "stranger@example.com",
            "invite_token": "aex_iv_A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status().as_u16(), 403);
    let bad = http
        .post(format!("{base}/v1/accounts"))
        .json(&json!({
            "email": "not an email",
            "invite_token": "aex_iv_A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status().as_u16(), 400);

    // Wrong credentials never pass.
    let no = http
        .get(format!("{base}/v1/balance"))
        .bearer_auth("aex_at_wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(no.status().as_u16(), 401);

    // Create an API key; the secret appears once and never in the list.
    let keyed = json_of(
        http.post(format!("{base}/v1/keys"))
            .bearer_auth(&at)
            .json(&json!({"name": "laptop"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("ApiKeyCreated", &keyed);
    let sk = keyed["secret"].as_str().unwrap().to_string();
    let key_list = json_of(
        http.get(format!("{base}/v1/keys"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("ApiKeyList", &key_list);
    assert!(
        !key_list.to_string().contains(&sk),
        "secrets never reappear"
    );

    // A customer-app Hand receives a short-lived scoped WebSocket grant. API Gateway callbacks
    // are admitted only through the authenticated integration and carry trusted connection
    // metadata to Brain. Brain derives tenant/client from the grant; Aex never supplies a tenant
    // header on this path. A managed-environment NAT source is rejected before forwarding.
    let grant = json_of(
        http.post(format!("{base}/v1/customer-environment/grants"))
            .bearer_auth(&sk)
            .json(&json!({"client_id": "orders-api"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_eq!(
        grant["url"],
        "wss://customer-environment.example.test/connect"
    );
    assert_eq!(
        grant["observation_url"],
        "https://api.example.test/v1/customer-environment/observations/chg_test"
    );
    assert!(
        !grant["observation_url"]
            .as_str()
            .unwrap()
            .contains(grant["observation_token"].as_str().unwrap()),
        "the observation bearer must never enter a URL or access log"
    );
    assert_eq!(grant["protocol"], "aex.grant.test");
    let connect = http
        .post(format!("{base}/v1/customer-environment/gateway"))
        .header("x-aex-apigateway-token", GATEWAY_TOKEN)
        .header("x-aex-connection-id", "connection-1")
        .header("x-aex-route-key", "$connect")
        .header("x-aex-request-id", "request-1")
        .header("x-aex-source-ip", "203.0.113.7")
        .header(header::SEC_WEBSOCKET_PROTOCOL, "aex.grant.test")
        .header("x-forwarded-for", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status().as_u16(), 204);
    assert_eq!(
        connect.headers()[header::SEC_WEBSOCKET_PROTOCOL],
        "aex.grant.test"
    );
    // Authorizer context is connect-only. A later frame is accepted through the trusted proxy
    // without that integration token, but Brain must verify the raw connection-bound proof before
    // it mutates registration or liveness.
    let send_frame = |proof: &'static str, request_id: &'static str| {
        http.post(format!("{base}/v1/customer-environment/gateway"))
            .header("x-aex-connection-id", "connection-1")
            .header("x-aex-route-key", "$default")
            .header("x-aex-request-id", request_id)
            .header("x-aex-source-ip", "203.0.113.7")
            .header("x-forwarded-for", "203.0.113.7")
            .json(&json!({"type":"heartbeat", "epoch":7, "proof":proof}))
            .send()
    };
    assert_eq!(
        send_frame("bound-frame-proof", "request-2")
            .await
            .unwrap()
            .status()
            .as_u16(),
        204
    );
    assert_eq!(
        send_frame("spoofed-frame-proof", "request-3")
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    // `$disconnect` has no cryptographic proof, so Aex drops it and Brain expires by heartbeat or
    // a confirmed outbound 410 instead of accepting unauthenticated state mutation.
    let disconnect = http
        .post(format!("{base}/v1/customer-environment/gateway"))
        .header("x-aex-connection-id", "connection-1")
        .header("x-aex-route-key", "$disconnect")
        .header("x-aex-request-id", "request-4")
        .header("x-aex-source-ip", "203.0.113.7")
        .header("x-forwarded-for", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(disconnect.status().as_u16(), 204);
    let observation = http
        .post(format!(
            "{base}/v1/customer-environment/observations/chg_test"
        ))
        .bearer_auth("observation-grant")
        .json(&json!({
            "type": "terminal",
            "epoch": 7,
            "operation_id": "op_test",
            "request_digest": "a".repeat(64),
            "ok": true,
            "output": {"ok": true}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(observation.status().as_u16(), 204);
    let missing_observation_token = http
        .post(format!(
            "{base}/v1/customer-environment/observations/chg_test"
        ))
        .json(&json!({
            "type": "terminal",
            "epoch": 7,
            "operation_id": "op_test",
            "request_digest": "a".repeat(64),
            "ok": true,
            "output": {"ok": true}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_observation_token.status().as_u16(), 401);
    let swapped_observation = http
        .post(format!(
            "{base}/v1/customer-environment/observations/chg_different"
        ))
        .bearer_auth("observation-grant")
        .json(&json!({
            "type": "terminal",
            "epoch": 7,
            "operation_id": "op_test",
            "request_digest": "a".repeat(64),
            "ok": true,
            "output": {"ok": true}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(swapped_observation.status().as_u16(), 401);
    let blocked = http
        .post(format!("{base}/v1/customer-environment/gateway"))
        .header("x-aex-apigateway-token", GATEWAY_TOKEN)
        .header("x-aex-connection-id", "connection-2")
        .header("x-aex-route-key", "$connect")
        .header("x-aex-request-id", "request-2")
        .header("x-aex-source-ip", "198.51.100.9")
        .header(header::SEC_WEBSOCKET_PROTOCOL, "aex.grant.test")
        .header("x-forwarded-for", "198.51.100.9")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status().as_u16(), 403);

    // Hosted admission requires a recoverable identity before it evaluates balance.
    let unidentified = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&sk)
        .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
        .send()
        .await
        .unwrap();
    let unidentified_body = json_of(unidentified, 400).await;
    assert_eq!(unidentified_body["error"]["code"], "invalid_request");

    // No money yet: identified session work is refused with the session-API error envelope.
    let refused = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "unfunded-create")
        .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
        .send()
        .await
        .unwrap();
    let refused_body = json_of(refused, 402).await;
    assert_eq!(refused_body["error"]["code"], "insufficient_balance");

    // Top up $10. Fake payments: pending at create, paid on first poll — the same
    // create/pay/poll/credit path the Stripe adapter drives.
    let topup = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(&at)
            .header("Idempotency-Key", "e2e-topup-key-02")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("Topup", &topup);
    assert_eq!(topup["status"], "pending");
    assert!(topup["checkout_url"].is_string());
    let below_min = http
        .post(format!("{base}/v1/topups"))
        .bearer_auth(&at)
        .header("Idempotency-Key", "e2e-topup-key-03")
        .json(&json!({"amount_cents": 999}))
        .send()
        .await
        .unwrap();
    assert_eq!(below_min.status().as_u16(), 400, "the $10 minimum holds");
    let above_max = http
        .post(format!("{base}/v1/topups"))
        .bearer_auth(&at)
        .header("Idempotency-Key", "e2e-topup-key-04")
        .json(&json!({"amount_cents": 100_001}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        above_max.status().as_u16(),
        400,
        "the $1,000 alpha maximum holds"
    );
    let topup_id = topup["id"].as_str().unwrap();
    let paid = json_of(
        http.get(format!("{base}/v1/topups/{topup_id}"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Topup", &paid);
    assert_eq!(paid["status"], "paid");
    let balance = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Balance", &balance);
    assert_eq!(balance["microusd"], "10000000");
    assert_eq!(balance["usd"], "10.00");

    // Support can return unused credit without a customer-side refund control. The operator
    // request reserves the ledger first and is idempotent across retries.
    let unauthenticated_refund = http
        .post(format!("{base}/v1/admin/refunds"))
        .header("Idempotency-Key", "e2e-refund-1")
        .json(&json!({"topup_id": topup_id, "amount_cents": 100}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated_refund.status().as_u16(), 401);
    let missing_key = http
        .post(format!("{base}/v1/admin/refunds"))
        .bearer_auth(OPERATOR_TOKEN)
        .json(&json!({"topup_id": topup_id, "amount_cents": 100}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_key.status().as_u16(), 400);
    let refunded = json_of(
        http.post(format!("{base}/v1/admin/refunds"))
            .bearer_auth(OPERATOR_TOKEN)
            .header("Idempotency-Key", "e2e-refund-1")
            .json(&json!({"topup_id": topup_id, "amount_cents": 100}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Refund", &refunded);
    assert_eq!(refunded["status"], "succeeded");
    let refund_id = refunded["id"].as_str().unwrap().to_owned();
    let retried = json_of(
        http.post(format!("{base}/v1/admin/refunds"))
            .bearer_auth(OPERATOR_TOKEN)
            .header("Idempotency-Key", "e2e-refund-1")
            .json(&json!({"topup_id": topup_id, "amount_cents": 100}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(retried["id"], refund_id);
    let changed_retry = http
        .post(format!("{base}/v1/admin/refunds"))
        .bearer_auth(OPERATOR_TOKEN)
        .header("Idempotency-Key", "e2e-refund-1")
        .json(&json!({"topup_id": topup_id, "amount_cents": 101}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed_retry.status().as_u16(), 409);
    let balance = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(balance["microusd"], "9000000");
    assert_eq!(balance["usd"], "9.00");

    // Hosted alpha has one physical managed-compute seal. Reject a raw neutral Brain shape before
    // reserving or dispatching so the control plane can neither promise nor underbill unavailable
    // capacity. The SDK omits this field entirely.
    let before_unsupported_shape = stub_brain.counter.load(Ordering::SeqCst);
    let unsupported_shape = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "unsupported-shape")
        .json(&json!({
            "model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"},
            "shape": "2gb"
        }))
        .send()
        .await
        .unwrap();
    let unsupported_shape_body = json_of(unsupported_shape, 422).await;
    assert_eq!(unsupported_shape_body["error"]["code"], "invalid_request");
    assert_eq!(
        stub_brain.counter.load(Ordering::SeqCst),
        before_unsupported_shape,
        "unsupported shapes must not reach Brain",
    );

    // Now a session runs. The official subagents Tool is an ordinary component with Brain's
    // native children grant; Aex must not rewrite it into a legacy Environment callback.
    let component_subagents = json!({
        "definition": {
            "name": "subagents",
            "description": "Create child sessions.",
            "input_schema": {"type": "object"},
            "output_schema": {},
            "contract_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        },
        "executor": {
            "kind": "component",
            "component_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "world": "aex:tool/tool@1.0.0",
            "config": {"definition": {"name": "subagents"}},
            "grants": ["children"]
        }
    });
    let create_request = json!({
        "model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"},
        "tools": {"items": [component_subagents.clone()]}
    });
    let session = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .header("Idempotency-Key", "same-create-request")
            .json(&create_request)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_valid(brain_protocol::SESSION_SCHEMA_JSON, "Session", &session);
    let forwarded = stub_brain.last_create_body.lock().unwrap().clone().unwrap();
    assert_eq!(forwarded["tools"]["items"][0], component_subagents);
    assert_eq!(
        forwarded["tools"]["items"][1]["executor"]["capability"],
        "aex.output"
    );
    assert!(forwarded.get("secrets").is_none());
    let sid = session["id"].as_str().unwrap().to_string();
    let oversized_upload = http
        .post(format!("{base}/v1/sessions/{sid}/storage/uploads"))
        .bearer_auth(&sk)
        .json(&json!({
            "key": "huge.bin",
            "bytes": 512_u64 * 1024 * 1024 + 1,
            "sha256": "00".repeat(32)
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized_upload.status().as_u16(), 413);
    assert_eq!(
        oversized_upload.json::<Value>().await.unwrap()["error"]["code"],
        "file_too_large"
    );

    let replay = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .header("Idempotency-Key", "same-create-request")
            .json(&create_request)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_eq!(
        replay["id"], sid,
        "a create replay returns the original session"
    );
    let changed_create = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "same-create-request")
        .json(&json!({"model": {"provider": "anthropic", "name": "changed", "api_key": "sk-x"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed_create.status().as_u16(), 409);

    // Ownership: a different account's key sees 404, not 403 — existence is not revealed.
    let other = invited_signup(&http, &base, "other@example.com").await;
    let other_key = json_of(
        http.post(format!("{base}/v1/keys"))
            .bearer_auth(other["account_token"].as_str().unwrap())
            .json(&json!({"name": "x"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let foreign = http
        .get(format!("{base}/v1/sessions/{sid}"))
        .bearer_auth(other_key["secret"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status().as_u16(), 404);
    // The public proxy rejects one byte beyond Brain's exact journal-record ceiling before
    // buffering, output preparation, admission, or an upstream call.
    let oversized_message = http
        .post(format!("{base}/v1/sessions/{sid}/messages"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "oversized-message")
        .header(header::CONTENT_TYPE, "application/json")
        .body(vec![b'x'; brain_protocol::MAX_MESSAGE_REQUEST_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(oversized_message.status().as_u16(), 413);

    // Hosted structured output adds its durable identity to Brain's acceptance. The proxy must
    // not retain Brain's shorter Content-Length after replacing that response body.
    let output_schema = json!({
        "type": "object",
        "properties": {"ok": {"const": "AEX_OUTPUT_OK"}},
        "required": ["ok"],
        "additionalProperties": false
    });
    let output_schema_hash =
        hex::encode(Sha256::digest(serde_jcs::to_vec(&output_schema).unwrap()));
    let typed_response = http
        .post(format!("{base}/v1/sessions/{sid}/messages"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "typed-output-message")
        .json(&json!({
            "content": "return structured output",
            "output": {
                "schema": output_schema,
                "schema_hash": output_schema_hash,
                "retries": 1
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(typed_response.status().as_u16(), 202);
    let declared_length = typed_response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    let typed_bytes = typed_response.bytes().await.unwrap();
    if let Some(declared_length) = declared_length {
        assert_eq!(declared_length, typed_bytes.len());
    }
    let accepted: Value = serde_json::from_slice(&typed_bytes).unwrap();
    assert!(accepted["output_id"].as_str().is_some());
    assert_eq!(accepted["schema_hash"], output_schema_hash);
    let mut neutral_accepted = accepted.clone();
    neutral_accepted
        .as_object_mut()
        .unwrap()
        .remove("output_id");
    neutral_accepted
        .as_object_mut()
        .unwrap()
        .remove("schema_hash");
    assert_valid(
        brain_protocol::SESSION_SCHEMA_JSON,
        "MessageAccepted",
        &neutral_accepted,
    );
    let unidentified_message = http
        .post(format!("{base}/v1/sessions/{sid}/messages"))
        .bearer_auth(&sk)
        .json(&json!({"content": "unnamed retry"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unidentified_message.status().as_u16(), 400);

    // Ordinary event bytes and SSE framing pass through; the reserved output protocol stays on
    // the operator stream so raw candidates are never exposed to customers.
    let events_text = http
        .get(format!(
            "{base}/v1/sessions/{sid}/events?after=0&follow=false"
        ))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert!(
        events_text
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream")
    );
    let events_text = events_text.text().await.unwrap();
    assert!(
        events_text.contains("event: turn.completed"),
        "{events_text}"
    );
    assert!(!events_text.contains("aex_submit_output"), "{events_text}");
    assert!(!events_text.contains("must-not-leak"), "{events_text}");

    // The list endpoint returns only this account's sessions, as contract Session documents.
    let listed = json_of(
        http.get(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_valid(brain_protocol::SESSION_SCHEMA_JSON, "SessionList", &listed);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    // The bill. 1.5 s on 1gb = 50 micro-USD compute, plus one successful managed search =
    // exactly 3,000 micro-USD. Both are folded from the committed event log.
    let usage = json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Usage", &usage);
    assert_eq!(usage["account_id"], account_id.as_str());
    let line = &usage["sessions"][0];

    assert_eq!(line["session_id"], sid.as_str());
    assert_eq!(line["running_ms"], "1500");
    assert_eq!(line["compute_microusd"], "50");
    assert_eq!(line["web_search_queries"], 1);
    assert_eq!(line["web_search_microusd"], "3000");
    let total = usage["total_microusd"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(total >= 3_050, "storage only adds: {total}");
    assert_eq!(
        usage["balance_microusd"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        9_000_000 - total,
        "balance = credits minus the rated total"
    );

    // Metering is idempotent: reading the bill again re-sweeps and must not double-bill.
    let usage2 = json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(usage2["sessions"][0]["running_ms"], "1500");
    assert_eq!(usage2["sessions"][0]["compute_microusd"], "50");
    assert_eq!(usage2["sessions"][0]["web_search_queries"], 1);
    assert_eq!(usage2["sessions"][0]["web_search_microusd"], "3000");

    // Brain's native subagent capability creates and ends ordinary children without touching an
    // Aex child endpoint. They happen entirely between sweeps and remain absent from the lagging
    // tenant GSI. One child delete is accepted immediately before its ancestor, exercising the
    // durable overlap scheduler; the other remains wholly hidden for strong-delete discovery.
    for index in 0..2 {
        json_of(
            http.post(format!("{base}/v1/sessions/{sid}/messages"))
                .bearer_auth(&sk)
                .header("Idempotency-Key", format!("native-child-{index}"))
                .json(&json!({"content": "spawn native child"}))
                .send()
                .await
                .unwrap(),
            202,
        )
        .await;
    }
    let children = json_of(
        http.get(format!("{base}/v1/sessions/{sid}/children"))
            .bearer_auth(&sk)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let child_id = children["data"][0]["id"].as_str().unwrap().to_owned();
    let child_deleted = http
        .delete(format!("{base}/v1/sessions/{child_id}"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert_eq!(child_deleted.status().as_u16(), 202);

    // DELETE only crosses Aex's durable acceptance boundary. The root job must wait behind the
    // already accepted descendant, then fence the remainder, strongly discover and settle every
    // still-present child journal, and only then allow Brain's cascading purge.
    let deleted = http
        .delete(format!("{base}/v1/sessions/{sid}"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status().as_u16(), 202);
    assert_eq!(
        deleted.headers()[header::LOCATION],
        format!("/v1/sessions/{sid}/deletion")
    );
    assert_eq!(deleted.headers()[header::RETRY_AFTER], "1");
    let work_after_acceptance = http
        .post(format!("{base}/v1/sessions/{sid}/messages"))
        .bearer_auth(&sk)
        .header("Idempotency-Key", "work-after-delete")
        .json(&json!({"content":"too late"}))
        .send()
        .await
        .unwrap();
    assert_eq!(work_after_acceptance.status().as_u16(), 409);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(16);
    loop {
        let status = json_of(
            http.get(format!("{base}/v1/sessions/{sid}/deletion"))
                .bearer_auth(&sk)
                .send()
                .await
                .unwrap(),
            200,
        )
        .await;
        if status["state"] == "succeeded" {
            assert!(status["completed_at_ms"].as_i64().is_some());
            break;
        }
        assert_eq!(status["state"], "deleting");
        assert!(
            tokio::time::Instant::now() < deadline,
            "durable deletion did not finish: {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let child_status = json_of(
        http.get(format!("{base}/v1/sessions/{child_id}/deletion"))
            .bearer_auth(&sk)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(child_status["state"], "succeeded");
    let repeated = http
        .delete(format!("{base}/v1/sessions/{sid}"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert_eq!(
        repeated.status().as_u16(),
        204,
        "confirmed deletion is idempotent without touching the absent Brain session"
    );
    let usage3 = json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Usage", &usage3);
    assert_eq!(usage3["sessions"].as_array().unwrap().len(), 3);
    let storage_rate = usage3["rates"]["session_storage_gb_month_microusd"]
        .as_str()
        .unwrap()
        .parse::<i128>()
        .unwrap();
    let storage_denominator = 1_000_000_000i128
        * i128::from(usage3["rates"]["month_hours"].as_i64().unwrap())
        * 3_600_000;
    for line in usage3["sessions"].as_array().unwrap() {
        let byte_milliseconds = line["session_storage_byte_milliseconds"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap();
        let storage_microusd = line["storage_microusd"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap();
        assert_eq!(
            storage_microusd,
            byte_milliseconds * storage_rate / storage_denominator,
            "the public exact meter must reproduce each session storage charge"
        );
    }
    let root_line = usage3["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["session_id"] == sid)
        .unwrap();
    assert_eq!(root_line["state"], "deleted");
    assert_eq!(root_line["running_ms"], "4500");
    assert_eq!(root_line["compute_microusd"], "150");
    assert_eq!(root_line["web_search_queries"], 3);
    let child_lines = usage3["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["session_id"] != sid)
        .collect::<Vec<_>>();
    assert_eq!(child_lines.len(), 2);
    assert!(
        child_lines.iter().all(|line| line["shape"] == "1gb"),
        "hidden ordinary children inherit the root's fixed hosted physical seal"
    );
    assert!(child_lines.iter().all(|line| line["state"] == "deleted"));
    assert!(child_lines.iter().all(|line| line["running_ms"] == "3600"));
    assert!(
        child_lines
            .iter()
            .all(|line| line["compute_microusd"] == "120")
    );
    assert!(
        child_lines
            .iter()
            .all(|line| line["session_storage_byte_milliseconds"] == "900500000000000")
    );
    assert!(
        child_lines
            .iter()
            .all(|line| line["storage_microusd"] == "10")
    );

    let usage4 = json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(&at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let children_again = usage4["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["session_id"] != sid)
        .collect::<Vec<_>>();
    assert_eq!(children_again.len(), 2);
    assert!(
        children_again
            .iter()
            .all(|line| line["running_ms"] == "3600")
    );
    assert!(
        children_again
            .iter()
            .all(|line| line["compute_microusd"] == "120")
    );
    assert!(
        children_again
            .iter()
            .all(|line| line["session_storage_byte_milliseconds"] == "900500000000000")
    );
    assert!(
        children_again
            .iter()
            .all(|line| line["storage_microusd"] == "10")
    );
    let listed_after = json_of(
        http.get(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(listed_after["data"].as_array().unwrap().len(), 0);

    // The public rate card needs no authentication.
    let rates = json_of(
        http.get(format!("{base}/v1/rates")).send().await.unwrap(),
        200,
    )
    .await;
    control_valid("RateCard", &rates);
    assert_eq!(rates["vcpu_hour_microusd"], "190000");
}

#[tokio::test(flavor = "multi_thread")]
async fn uncertain_refund_keeps_credit_reserved_and_retries_safely() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let payments = Arc::new(FlakyRefundPayments {
        refund_attempts: AtomicUsize::new(0),
    });
    let base =
        spawn_control_with_payments(&brain_url, brain_token, (10, 30), payments.clone()).await;
    let http = reqwest::Client::new();
    let created = invited_signup(&http, &base, "refund@example.com").await;
    let account_token = created["account_token"].as_str().unwrap();
    let topup = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(account_token)
            .header("Idempotency-Key", "e2e-topup-key-05")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let topup_id = topup["id"].as_str().unwrap();
    json_of(
        http.get(format!("{base}/v1/topups/{topup_id}"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;

    let first = http
        .post(format!("{base}/v1/admin/refunds"))
        .bearer_auth(OPERATOR_TOKEN)
        .header("Idempotency-Key", "uncertain-refund-1")
        .json(&json!({"topup_id": topup_id, "amount_cents": 1000}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status().as_u16(), 502);
    let held = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(held["microusd"], "0", "uncertain money cannot be spent");

    let retry = json_of(
        http.post(format!("{base}/v1/admin/refunds"))
            .bearer_auth(OPERATOR_TOKEN)
            .header("Idempotency-Key", "uncertain-refund-1")
            .json(&json!({"topup_id": topup_id, "amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Refund", &retry);
    assert_eq!(retry["status"], "succeeded");
    assert_eq!(payments.refund_attempts.load(Ordering::SeqCst), 2);
    let held = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(held["microusd"], "0", "retry never removes credit twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_credit_grants_are_auditable_and_idempotent() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();
    let created = invited_signup(&http, &base, "credit@example.com").await;
    let account_token = created["account_token"].as_str().unwrap();
    let account_id = created["account"]["id"].as_str().unwrap();
    let request = json!({
        "email": "credit@example.com",
        "amount_cents": 500,
        "reason": "Alpha evaluation"
    });

    let unauthenticated = http
        .post(format!("{base}/v1/admin/credit-grants"))
        .header("Idempotency-Key", "e2e-credit-grant-unauthenticated")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status().as_u16(), 401);

    let missing_key = http
        .post(format!("{base}/v1/admin/credit-grants"))
        .bearer_auth(OPERATOR_TOKEN)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(missing_key.status().as_u16(), 400);

    let grant = json_of(
        http.post(format!("{base}/v1/admin/credit-grants"))
            .bearer_auth(OPERATOR_TOKEN)
            .header("Idempotency-Key", "e2e-credit-grant-1")
            .json(&request)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("CreditGrant", &grant);
    assert_eq!(grant["account_id"], account_id);
    assert_eq!(grant["email"], "credit@example.com");
    assert_eq!(grant["amount_cents"], 500);
    assert_eq!(grant["reason"], "Alpha evaluation");

    let replay = json_of(
        http.post(format!("{base}/v1/admin/credit-grants"))
            .bearer_auth(OPERATOR_TOKEN)
            .header("Idempotency-Key", "e2e-credit-grant-1")
            .json(&request)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("CreditGrant", &replay);
    assert_eq!(replay["id"], grant["id"]);

    let mismatched_replay = http
        .post(format!("{base}/v1/admin/credit-grants"))
        .bearer_auth(OPERATOR_TOKEN)
        .header("Idempotency-Key", "e2e-credit-grant-1")
        .json(&json!({
            "email": "credit@example.com",
            "amount_cents": 501,
            "reason": "Alpha evaluation"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatched_replay.status().as_u16(), 409);

    let unknown_account = http
        .post(format!("{base}/v1/admin/credit-grants"))
        .bearer_auth(OPERATOR_TOKEN)
        .header("Idempotency-Key", "e2e-credit-grant-unknown")
        .json(&json!({
            "email": "unknown@example.com",
            "amount_cents": 500,
            "reason": "Alpha evaluation"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_account.status().as_u16(), 404);

    let balance = json_of(
        http.get(format!("{base}/v1/balance"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("Balance", &balance);
    assert_eq!(balance["microusd"], "5000000");

    let topups = json_of(
        http.get(format!("{base}/v1/topups"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    control_valid("TopupList", &topups);
    assert_eq!(topups["data"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn abuse_caps_hold_concurrency_and_create_rate() {
    let brain_token = "operator-token";
    let (brain_url, stub) = spawn_stub_brain_with_state(brain_token).await;
    // One live root at a time, two root creates per hour.
    let base = spawn_control(&brain_url, brain_token, (1, 2)).await;
    let http = reqwest::Client::new();

    let created = invited_signup(&http, &base, "capped@example.com").await;
    let at = created["account_token"].as_str().unwrap();
    let keyed = json_of(
        http.post(format!("{base}/v1/keys"))
            .bearer_auth(at)
            .json(&json!({"name": "k"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let sk = keyed["secret"].as_str().unwrap().to_string();
    let topup = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(at)
            .header("Idempotency-Key", "e2e-topup-key-06")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    json_of(
        http.get(format!(
            "{base}/v1/topups/{}",
            topup["id"].as_str().unwrap()
        ))
        .bearer_auth(at)
        .send()
        .await
        .unwrap(),
        200,
    )
    .await;

    let create_sequence = AtomicUsize::new(0);
    let create = || async {
        let key = format!(
            "abuse-create-{}",
            create_sequence.fetch_add(1, Ordering::Relaxed)
        );
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .header("Idempotency-Key", key)
            .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
            .send()
            .await
            .unwrap()
    };
    let first = json_of(create().await, 201).await;
    let sid = first["id"].as_str().unwrap().to_string();

    // Second root create while the first root is live: the concurrency cap bites.
    let capped = create().await;
    assert_eq!(capped.status().as_u16(), 429);
    let body: Value = capped.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");

    // A failed lifecycle is non-admitting, but it is not proof that the root's sandbox resources
    // were released. Force the authoritative projection through a strong usage fold and ensure
    // neither the refreshed GSI count nor Aex's durable local floor opens a replacement slot.
    {
        let mut sessions = stub.sessions.lock().unwrap();
        let failed = sessions.get_mut(&sid).unwrap();
        failed.doc["state"] = json!("failed");
        failed.doc["updated_at"] = json!(aex_control::rfc3339(aex_control::now_ms()));
    }
    json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(
        create().await.status().as_u16(),
        429,
        "a strongly observed failed root remains resource-bearing until end or deletion"
    );

    // End is only short acceptance. The slot remains held until a strong settlement observes the
    // ordinary root as ended; the eventual lifecycle GSI alone is not a safe release proof.
    let ended = http
        .post(format!("{base}/v1/sessions/{sid}/end"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    let ended = json_of(ended, 202).await;
    assert_eq!(ended["state"], "ending");
    assert_eq!(
        create().await.status().as_u16(),
        429,
        "an eventual open-to-ending index gap must not free the durable local root slot"
    );
    json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(at)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    // Once strongly observed ended, root concurrency frees independently of Hand capacity...
    json_of(create().await, 201).await;

    // ...but the third root create in the hour hits the create-rate cap.
    let rate_capped = create().await;
    assert_eq!(rate_capped.status().as_u16(), 429);
}

#[tokio::test(flavor = "multi_thread")]
async fn retained_root_cap_requires_confirmed_physical_deletion() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control_with_product_limits(
        &brain_url,
        brain_token,
        (1, 3),
        1,
        Arc::new(FakePayments),
    )
    .await;
    let http = reqwest::Client::new();

    let created = invited_signup(&http, &base, "retained-cap@example.com").await;
    let account_token = created["account_token"].as_str().unwrap();
    let key = json_of(
        http.post(format!("{base}/v1/keys"))
            .bearer_auth(account_token)
            .json(&json!({"name": "retained-cap"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let api_key = key["secret"].as_str().unwrap();
    let topup = json_of(
        http.post(format!("{base}/v1/topups"))
            .bearer_auth(account_token)
            .header("Idempotency-Key", "e2e-topup-key-07")
            .json(&json!({"amount_cents": 1000}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    json_of(
        http.get(format!(
            "{base}/v1/topups/{}",
            topup["id"].as_str().unwrap()
        ))
        .bearer_auth(account_token)
        .send()
        .await
        .unwrap(),
        200,
    )
    .await;

    let request = json!({
        "model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}
    });
    let first = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(api_key)
            .header("Idempotency-Key", "retained-first")
            .json(&request)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let session_id = first["id"].as_str().unwrap();
    json_of(
        http.post(format!("{base}/v1/sessions/{session_id}/end"))
            .bearer_auth(api_key)
            .send()
            .await
            .unwrap(),
        202,
    )
    .await;
    // Strong reconciliation observes `ended`, releasing the one concurrent slot but not the
    // independent retained-journal slot.
    json_of(
        http.get(format!("{base}/v1/usage"))
            .bearer_auth(account_token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;

    let blocked = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(api_key)
        .header("Idempotency-Key", "retained-replacement")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status().as_u16(), 429);
    let blocked: Value = blocked.json().await.unwrap();
    assert_eq!(blocked["error"]["code"], "rate_limited");
    assert!(
        blocked["error"]["message"]
            .as_str()
            .unwrap()
            .contains("await session.delete()"),
        "quota recovery must be explicit: {blocked}",
    );

    let accepted = http
        .delete(format!("{base}/v1/sessions/{session_id}"))
        .bearer_auth(api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status().as_u16(), 202);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(16);
    loop {
        let status = json_of(
            http.get(format!("{base}/v1/sessions/{session_id}/deletion"))
                .bearer_auth(api_key)
                .send()
                .await
                .unwrap(),
            200,
        )
        .await;
        if status["state"] == "succeeded" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{status}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(api_key)
            .header("Idempotency-Key", "retained-replacement")
            .json(&request)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
}
