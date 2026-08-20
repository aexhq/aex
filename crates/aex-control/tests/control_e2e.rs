//! The slice-4 gate, in CI form: a stranger signs up, tops up, creates a key, runs a session,
//! sees the bill — over real HTTP against the control router, with a stub brain implementing
//! session/v1 from the contracts and the fake payments adapter. Every control-plane response
//! is validated against `contracts/control/v1/schemas.json`; the proxied session documents are
//! validated against Brain's owned session schema. The real-wire version of this flow
//! (real brain, real Stripe test mode) is `tools/m1.sh`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aex_control::api::{AppState, internal_router, router};
use aex_control::brain::BrainClient;
use aex_control::payments::{Checkout, FakePayments, PaymentStatus, Payments, RefundAttempt};
use aex_control::rating::RateCard;
use aex_control::store::Db;
use aex_control::web::WebRuntime;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri, header};
use axum::response::Response;
use serde_json::{Value, json};

const OPERATOR_TOKEN: &str = "aex_ad_A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2A1b2";
const EXECUTOR_TOKEN: &str = "brain-to-aex-private-test-token";

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
    body: Bytes,
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
            if let Some(key) = key {
                assert!(
                    key.starts_with("aex:acc_") && !key.contains("same-create-request"),
                    "the control plane must namespace and hash the customer key"
                );
                stub.create_requests
                    .lock()
                    .unwrap()
                    .insert(key, (request_hash, id));
            }
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
        create_requests: Mutex::new(HashMap::new()),
    });
    let app = axum::Router::new().fallback(stub_handler).with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
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
    let state = AppState {
        db: Db::open_memory().unwrap(),
        brain: BrainClient::new(brain_url, brain_token),
        payments,
        stripe_webhook: None,
        card: RateCard::default(),
        operator_token_hash: Some(aex_control::identity::hash_secret(OPERATOR_TOKEN)),
        external_executor_token_hash: None,
        web: WebRuntime::hosted(None),
        default_limits: limits,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
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
        web: WebRuntime::hosted(None),
        default_limits: (10, 30),
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
        "context": {"brain.capability": "aex.web.fetch.v1"}
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
    assert_eq!(balance["microusd"], 10_000_000);

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
async fn a_stranger_signs_up_tops_up_keys_runs_and_sees_the_bill() {
    let brain_token = "operator-token";
    let brain_url = spawn_stub_brain(brain_token).await;
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
    let above_max = http
        .post(format!("{base}/v1/topups"))
        .bearer_auth(&at)
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
    assert_eq!(balance["microusd"], 10_000_000);
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
    assert_eq!(balance["microusd"], 9_000_000);
    assert_eq!(balance["usd"], "9.00");

    // Now a session runs.
    let session = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .header("Idempotency-Key", "same-create-request")
            .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_valid(brain_protocol::SESSION_SCHEMA_JSON, "Session", &session);
    let sid = session["id"].as_str().unwrap().to_string();

    let replay = json_of(
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(&sk)
            .header("Idempotency-Key", "same-create-request")
            .json(&json!({"model": {"provider": "anthropic", "name": "m", "api_key": "sk-x"}}))
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
        brain_protocol::SESSION_SCHEMA_JSON,
        "MessageAccepted",
        &accepted,
    );

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
    assert_eq!(line["running_ms"], 1_500);
    assert_eq!(line["compute_microusd"], 50);
    assert_eq!(line["web_search_queries"], 1);
    assert_eq!(line["web_search_microusd"], 3_000);
    assert_eq!(line["storage"]["suspended_bytes"], 1_073_741_824);
    let total = usage["total_microusd"].as_i64().unwrap();
    assert!(total >= 3_050, "storage only adds: {total}");
    assert_eq!(
        usage["balance_microusd"].as_i64().unwrap(),
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
    assert_eq!(held["microusd"], 0, "uncertain money cannot be spent");

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
    assert_eq!(held["microusd"], 0, "retry never removes credit twice");
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
    assert_eq!(balance["microusd"], 5_000_000);

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
    let brain_url = spawn_stub_brain(brain_token).await;
    // One live session at a time, two creates per hour.
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
