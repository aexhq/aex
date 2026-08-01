//! The no-dispatch proof and the transport's refusals, against a real server
//! (plan 08 §10 slices S-3.3, S-3.5, S-3.6).
//!
//! Every case here answers one question: **did a request reach the socket?**
//! `wiremock` records every request it receives, so "zero requests observed" is
//! evidence rather than an assertion about our own code.

use aex_brain_provider_gateway::budget::StreamBudget;
use aex_brain_provider_gateway::credential::ProviderApiKey;
use aex_brain_provider_gateway::transport::{
    Accept, AuthScheme, ExecuteError, SendState, WireRequest, execute,
};
use aex_brain_provider_gateway::wire_pending::DispatchProof;
use aex_model_catalog::document::EndpointPin;
use aex_model_catalog::primitives::BoundedString;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A key shaped like a real one, so the redaction assertions are meaningful.
const KEY: &str = "sk-test-0123456789abcdefghij0123456789abcdefghij";

fn request(path: &str, auth: AuthScheme) -> WireRequest {
    WireRequest {
        endpoint: EndpointPin::OpenAiApi,
        path: BoundedString::new(path).expect("path"),
        query: Vec::new(),
        headers: Vec::new(),
        auth,
        body: bytes::Bytes::from_static(b"{\"stream\":true}"),
        accept: Accept::TextEventStream,
    }
}

/// A client pointed at the mock rather than at a pinned origin.
///
/// The pinned origins are compiled and unreachable from configuration, so a
/// protocol test cannot use one. What it *can* prove is everything that happens
/// on either side of the gate, which is what these cases are about.
fn client(budget: &StreamBudget) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(budget.connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("a test client builds")
}

async fn observed(server: &MockServer) -> usize {
    server.received_requests().await.map_or(0, |all| all.len())
}

async fn send_to(server: &MockServer, built: &WireRequest) -> (u16, usize) {
    let budget = StreamBudget::default();
    let client = client(&budget);
    let key = ProviderApiKey::new(KEY.to_owned());
    let mut state = SendState::new();
    let url = format!("{}{}", server.uri(), built.path.as_str());
    let (name, value) = key
        .sensitive_header(built.auth)
        .expect("the key becomes a header");
    let prepared = client
        .post(&url)
        .header("accept", built.accept.as_str())
        .header("content-type", "application/json")
        .header(name, value)
        .body(built.body.clone());
    state.send().expect("the gate opens exactly once");
    let response = prepared.send().await.expect("the mock answers");
    (response.status().as_u16(), observed(server).await)
}

// ---------------------------------------------------------------------------
// S-3.6 the no-dispatch proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_url_assembly_failure_reaches_no_socket() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut broken = request("/v1/responses", AuthScheme::BearerAuthorization);
    // A path `Url::join` cannot resolve against the compiled origin.
    broken.path = BoundedString::new("http://[").expect("path");
    let key = ProviderApiKey::new(KEY.to_owned());
    let mut state = SendState::new();
    let budget = StreamBudget::default();

    let error = execute(&client(&budget), &broken, &key, &mut state)
        .await
        .expect_err("a malformed path must not reach a socket");
    assert_eq!(error.proof(), DispatchProof::NotSent);
    assert_eq!(state.proof(), DispatchProof::NotSent);
    assert_eq!(
        observed(&server).await,
        0,
        "the mock observed a request for a failure that claims NotSent"
    );
}

#[tokio::test]
async fn a_credential_that_cannot_become_a_header_reaches_no_socket() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    // A newline in the material cannot enter a header value.
    let key = ProviderApiKey::new("sk-\nInjected: value".to_owned());
    let mut state = SendState::new();
    let budget = StreamBudget::default();

    let error = execute(&client(&budget), &built, &key, &mut state)
        .await
        .expect_err("an illegal header byte must not reach a socket");
    assert_eq!(error.proof(), DispatchProof::NotSent);
    assert_eq!(state.proof(), DispatchProof::NotSent);
    assert_eq!(observed(&server).await, 0);
}

#[tokio::test]
async fn a_second_send_on_one_attempt_reaches_no_socket() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    let key = ProviderApiKey::new(KEY.to_owned());
    let mut state = SendState::new();
    // Burn the gate as the first attempt would.
    state.send().expect("the first send opens the gate");

    let budget = StreamBudget::default();
    let error = execute(&client(&budget), &built, &key, &mut state)
        .await
        .expect_err("a second send must be refused");
    assert!(matches!(error, ExecuteError::AlreadySent(_)));
    assert_eq!(
        observed(&server).await,
        0,
        "the second attempt reached the socket, which would be a second generation"
    );
}

#[tokio::test]
async fn a_post_gate_transport_failure_is_possibly_sent_not_not_sent() {
    // Port 1 on the loopback interface has no listener, so the connection fails
    // after the gate has been consumed. The honest answer is then ambiguous.
    let budget = StreamBudget::default();
    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    let mut state = SendState::new();

    let outcome = client(&budget)
        .post(format!("http://127.0.0.1:1{}", built.path.as_str()))
        .body(built.body.clone())
        .send()
        .await;
    let failure = outcome.expect_err("the connection must actually fail");

    // The same failure, expressed through the type the gate produces.
    state.send().expect("the gate opens");
    let error = ExecuteError::Transport {
        detail: BoundedString::truncating(&failure.to_string()),
    };
    assert_eq!(
        error.proof(),
        DispatchProof::PossiblySent,
        "a connect error after the gate is ambiguous, never NotSent"
    );
    assert_eq!(state.proof(), DispatchProof::PossiblySent);
    assert!(
        !error.to_string().contains(KEY),
        "the transport error carried the key"
    );
}

#[tokio::test]
async fn a_successful_send_reaches_the_socket_exactly_once() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("data: hi\n\n"))
        .mount(&server)
        .await;

    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    let (status, count) = send_to(&server, &built).await;
    assert_eq!(status, 200);
    assert_eq!(count, 1, "exactly one request per attempt");
}

// ---------------------------------------------------------------------------
// S-3.3 redirects, S-3.5 bounded error bodies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://example.invalid/moved"),
        )
        .mount(&server)
        .await;

    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    let (status, count) = send_to(&server, &built).await;
    assert_eq!(
        status, 302,
        "the redirect must surface as a status, not be chased"
    );
    assert_eq!(count, 1, "the hop was followed");
}

#[tokio::test]
async fn an_error_body_is_read_to_the_bound_and_no_further() {
    let server = MockServer::start().await;
    let oversized = "x".repeat(1024 * 1024);
    Mock::given(any())
        .respond_with(ResponseTemplate::new(400).set_body_string(oversized))
        .mount(&server)
        .await;

    let budget = StreamBudget::default();
    let mut response = client(&budget)
        .post(format!("{}/v1/responses", server.uri()))
        .body("{}")
        .send()
        .await
        .expect("the response arrives");
    assert_eq!(response.status().as_u16(), 400);

    let limit = budget.max_error_body_bytes as usize;
    let mut read = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.expect("chunk") {
        let remaining = limit.saturating_sub(read.len());
        if chunk.len() > remaining {
            read.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        read.extend_from_slice(&chunk);
    }
    assert!(truncated, "a 1 MiB body must trip the bound");
    assert_eq!(read.len(), limit, "the read stopped exactly at the bound");
}

// ---------------------------------------------------------------------------
// credential leak: the wire itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_credential_travels_only_in_the_sensitive_header() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let built = request("/v1/responses", AuthScheme::BearerAuthorization);
    let (_, count) = send_to(&server, &built).await;
    assert_eq!(count, 1);

    let requests = server.received_requests().await.expect("recorded");
    let recorded: &Request = &requests[0];

    assert!(
        !recorded.url.as_str().contains("sk-"),
        "the key reached the URL: {}",
        recorded.url
    );
    assert!(
        !String::from_utf8_lossy(&recorded.body).contains("sk-"),
        "the key reached the body"
    );
    let authorization = recorded
        .headers
        .get("authorization")
        .expect("the header is present");
    assert_eq!(
        authorization.to_str().expect("ascii"),
        format!("Bearer {KEY}"),
        "the key travels in exactly one place"
    );
    for (name, value) in &recorded.headers {
        if name.as_str() == "authorization" {
            continue;
        }
        assert!(
            !value.to_str().unwrap_or_default().contains("sk-"),
            "the key also reached `{name}`"
        );
    }
}

#[tokio::test]
async fn a_provider_that_echoes_the_key_into_an_error_body_cannot_leak_it() {
    let server = MockServer::start().await;
    let echoed = format!("{{\"error\":{{\"message\":\"invalid key Bearer {KEY}\"}}}}");
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401).set_body_string(echoed))
        .mount(&server)
        .await;

    let budget = StreamBudget::default();
    let body = client(&budget)
        .post(format!("{}/v1/responses", server.uri()))
        .body("{}")
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");
    assert!(body.contains(KEY), "the fixture must actually echo the key");

    let redacted = aex_brain_provider_gateway::redact::redact::<512>(&body, &[KEY]);
    assert!(
        !redacted.as_str().contains(KEY),
        "the redactor let the key through: {redacted}"
    );
    assert!(!redacted.as_str().contains("sk-test"));
}
