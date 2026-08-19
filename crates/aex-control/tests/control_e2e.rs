//! The slice-4 gate, in CI form: a stranger signs up, tops up, creates a key, runs a session,
//! sees the bill — over real HTTP against the control router, with a stub brain implementing
//! session/v1 from the contracts and the fake payments adapter. Every control-plane response
//! is validated against `contracts/control/v1/schemas.json`; the proxied session documents are
//! validated against `contracts/session/v1/schemas.json`. The real-wire version of this flow
//! (real brain, real Stripe test mode) is `tools/m1.sh`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use aex_control::api::{AppState, router};
use aex_control::brain::BrainClient;
use aex_control::payments::FakePayments;
use aex_control::rating::RateCard;
use aex_control::store::Db;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri, header};
use axum::response::Response;
use serde_json::{Value, json};

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

fn stub_doc(id: &str, state: &str, hand_state: &str, storage: Value, turns: i64) -> Value {
    json!({
        "id": id,
        "object": "session",
        "state": state,
        "model": {"provider": "anthropic", "name": "stub-model"},
        "hand": {"state": hand_state, "shape": "1gb"},
        "storage": storage,
        "created_at": "2026-08-18T09:00:00Z",
        "updated_at": "2026-08-18T09:00:00Z",
        "turns": turns,
        "metadata": {}
    })
}

async fn stub_handler(
    State(stub): State<Arc<StubBrain>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        auth,
        format!("Bearer {}", stub.token),
        "the proxy must authenticate to the brain with the operator token"
    );
    let path = uri.path().to_string();
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
        ("POST", ["v1", "sessions"]) => {
            let n = stub.counter.fetch_add(1, Ordering::SeqCst);
            let id = format!("ses_stub{n:020}");
            let doc = stub_doc(
                &id,
                "idle",
                "preparing",
                json!({"workspace_bytes": 0, "suspended_bytes": 0, "artifact_bytes": 0}),
                0,
            );
            sessions.insert(
                id.clone(),
                StubSession {
                    doc: doc.clone(),
                    events: Vec::new(),
                    seq: 0,
                    turns: 0,
                },
            );
            respond(201, doc)
        }
        ("GET", ["v1", "sessions", id]) => match sessions.get(*id) {
            Some(s) => respond(200, s.doc.clone()),
            None => respond(
                404,
                json!({"error": {"code": "not_found", "message": "no such session"}}),
            ),
        },
        ("POST", ["v1", "sessions", id, "messages"]) => {
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
            ev(
                "assistant.message",
                t0 + 1_400,
                json!({"agent_id": "root", "text": "done"}),
            );
            ev(
                "turn.completed",
                t0 + 1_500,
                json!({"stop_reason": "end_turn", "rounds": 1, "tool_calls": 1}),
            );
            s.turns += 1;
            s.doc = stub_doc(
                id,
                "idle",
                "suspended",
                json!({"workspace_bytes": 1000000, "suspended_bytes": 1073741824, "artifact_bytes": 0}),
                s.turns,
            );
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
            let after: i64 = uri
                .query()
                .and_then(|q| {
                    q.split('&')
                        .find_map(|kv| kv.strip_prefix("after="))
                        .and_then(|v| v.parse().ok())
                })
                .unwrap_or(0);
            let visible: Vec<Value> = s
                .events
                .iter()
                .filter(|e| e["seq"].as_i64().unwrap_or(0) > after)
                .cloned()
                .collect();
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(sse(&visible)))
                .unwrap()
        }
        ("POST", ["v1", "sessions", id, "end"]) => {
            let Some(s) = sessions.get_mut(*id) else {
                return respond(
                    404,
                    json!({"error": {"code": "not_found", "message": "no such session"}}),
                );
            };
            s.doc["hand"]["state"] = json!("released");
            s.doc["storage"]["suspended_bytes"] = json!(0);
            let doc = s.doc.clone();
            respond(200, doc)
        }
        ("DELETE", ["v1", "sessions", id]) => {
            if sessions.remove(*id).is_some() {
                Response::builder().status(204).body(Body::empty()).unwrap()
            } else {
                respond(
                    404,
                    json!({"error": {"code": "not_found", "message": "no such session"}}),
                )
            }
        }
        _ => respond(
            404,
            json!({"error": {"code": "not_found", "message": format!("stub: {method} {path}")}}),
        ),
    }
}

async fn spawn_stub_brain(token: &str) -> String {
    let stub = Arc::new(StubBrain {
        token: token.to_string(),
        counter: AtomicI64::new(0),
        sessions: Mutex::new(HashMap::new()),
    });
    let app = axum::Router::new().fallback(stub_handler).with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn spawn_control(brain_url: &str, brain_token: &str, limits: (i64, i64)) -> String {
    let state = AppState {
        db: Db::open_memory().unwrap(),
        brain: BrainClient::new(brain_url, brain_token),
        payments: Arc::new(FakePayments),
        stripe_webhook: None,
        card: RateCard::default(),
        default_limits: limits,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    base
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

// ---- the gate flow ----

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_signs_up_tops_up_keys_runs_and_sees_the_bill() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    let base = spawn_control(&brain_url, brain_token, (10, 30)).await;
    let http = reqwest::Client::new();

    // Sign up. The account token appears once.
    let created = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": "stranger@example.com"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    control_valid("AccountCreated", &created);
    let at = created["account_token"].as_str().unwrap().to_string();
    let account_id = created["account"]["id"].as_str().unwrap().to_string();

    // A second signup with the same email is a conflict; a garbage email fails fast.
    let dup = http
        .post(format!("{base}/v1/accounts"))
        .json(&json!({"email": "stranger@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status().as_u16(), 409);
    let bad = http
        .post(format!("{base}/v1/accounts"))
        .json(&json!({"email": "not an email"}))
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

    // No money yet: session work is refused with the session-API error envelope.
    let refused = http
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(&sk)
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
        .json(&json!({"amount_cents": 999}))
        .send()
        .await
        .unwrap();
    assert_eq!(below_min.status().as_u16(), 400, "the $10 minimum holds");
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
    assert_eq!(balance["microusd"], 10_000_000);
    assert_eq!(balance["usd"], "10.00");

    // Now a session runs.
    let session = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_valid(aex_contracts::SESSION_SCHEMA_JSON, "Session", &session);
    let sid = session["id"].as_str().unwrap().to_string();

    // Ownership: a different account's key sees 404, not 403 — existence is not revealed.
    let other = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": "other@example.com"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
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

    // A message; the stub journals a 1.5 s turn.
    let accepted = json_of(
        http.post(format!("{base}/v1/sessions/{sid}/messages"))
            .bearer_auth(&sk)
            .json(&json!({"content": "run"}))
            .send()
            .await
            .unwrap(),
        202,
    )
    .await;
    assert_valid(
        aex_contracts::SESSION_SCHEMA_JSON,
        "MessageAccepted",
        &accepted,
    );

    // The event stream passes through the proxy verbatim (SSE framing intact).
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
    assert_valid(aex_contracts::SESSION_SCHEMA_JSON, "SessionList", &listed);
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
    assert_eq!(line["running_ms"], 1_500);
    assert_eq!(line["compute_microusd"], 50);
    assert_eq!(line["web_search_queries"], 1);
    assert_eq!(line["web_search_microusd"], 3_000);
    assert_eq!(line["storage"]["suspended_bytes"], 1_073_741_824);
    let total = usage["total_microusd"].as_i64().unwrap();
    assert!(total >= 3_050, "storage only adds: {total}");
    assert_eq!(
        usage["balance_microusd"].as_i64().unwrap(),
        10_000_000 - total,
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
    assert_eq!(usage2["sessions"][0]["running_ms"], 1_500);
    assert_eq!(usage2["sessions"][0]["compute_microusd"], 50);
    assert_eq!(usage2["sessions"][0]["web_search_queries"], 1);
    assert_eq!(usage2["sessions"][0]["web_search_microusd"], 3_000);

    // Delete: irreversible at the brain; the control plane goes final, the bill survives.
    let deleted = http
        .delete(format!("{base}/v1/sessions/{sid}"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status().as_u16(), 204);
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
    assert_eq!(usage3["sessions"][0]["state"], "deleted");
    assert_eq!(usage3["sessions"][0]["running_ms"], 1_500);
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

    // The public rate card needs no auth and is the D4 card.
    let rates = json_of(
        http.get(format!("{base}/v1/rates")).send().await.unwrap(),
        200,
    )
    .await;
    control_valid("RateCard", &rates);
    assert_eq!(rates["vcpu_hour_microusd"], 190_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn abuse_caps_hold_concurrency_and_create_rate() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
    // One live session at a time, two creates per hour.
    let base = spawn_control(&brain_url, brain_token, (1, 2)).await;
    let http = reqwest::Client::new();

    let created = json_of(
        http.post(format!("{base}/v1/accounts"))
            .json(&json!({"email": "capped@example.com"}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
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

    let create = || async {
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
            .send()
            .await
            .unwrap()
    };
    let first = json_of(create().await, 201).await;
    let sid = first["id"].as_str().unwrap().to_string();

    // Second create while the first hand is live: the concurrency cap bites.
    let capped = create().await;
    assert_eq!(capped.status().as_u16(), 429);
    let body: Value = capped.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");

    // End the first session (hand released) — concurrency frees up...
    let ended = http
        .post(format!("{base}/v1/sessions/{sid}/end"))
        .bearer_auth(&sk)
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status().as_u16(), 200);
    json_of(create().await, 201).await;

    // ...but the third create in the hour hits the create-rate cap.
    let rate_capped = create().await;
    assert_eq!(rate_capped.status().as_u16(), 429);
}
