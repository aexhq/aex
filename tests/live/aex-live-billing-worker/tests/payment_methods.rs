//! Live saved-card lifecycle through the direct ingest and public read model.

use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Card {
    id: String,
    last4: String,
    expiry_month: u32,
    expiry_year: u32,
}

#[derive(Debug, Deserialize)]
struct CardPage {
    items: Vec<Card>,
}

fn required(name: &str) -> String {
    aex_test_harness::required_env!(name)
}

fn request(name: &str) -> serde_json::Value {
    serde_json::from_str(&required(name))
        .unwrap_or_else(|error| panic!("{name} must be a typed ingest request: {error}"))
}

fn display(request: &serde_json::Value) -> (&str, u64, u64) {
    let facts = &request["event"]["facts"];
    (
        facts["last4"].as_str().expect("last4 fixture"),
        facts["expiryMonth"].as_u64().expect("expiry month fixture"),
        facts["expiryYear"].as_u64().expect("expiry year fixture"),
    )
}

async fn invoke(
    client: &aws_sdk_lambda::Client,
    function: &str,
    request: &serde_json::Value,
    expected: &str,
) {
    let answer = client
        .invoke()
        .function_name(function)
        .invocation_type(InvocationType::RequestResponse)
        .payload(Blob::new(
            serde_json::to_vec(request).expect("fixture request encodes"),
        ))
        .send()
        .await
        .expect("finance-ingest is reachable");
    assert!(answer.function_error().is_none(), "Lambda function error");
    let payload = answer.payload().expect("finance-ingest returns a receipt");
    let receipt: serde_json::Value =
        serde_json::from_slice(payload.as_ref()).expect("typed ingest receipt");
    assert_eq!(receipt["result"], "accepted");
    assert_eq!(receipt["applied"], expected);
    assert!(
        receipt.get("transactionId").is_none(),
        "display projection must not post money"
    );
}

async fn cards(client: &reqwest::Client, api: &str, bearer: &str) -> CardPage {
    let response = client
        .get(format!(
            "{}/api/billing/payment-methods",
            api.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .send()
        .await
        .expect("central billing API is reachable");
    assert!(
        response.status().is_success(),
        "payment-method list succeeds"
    );
    response.json().await.expect("typed payment-method page")
}

#[tokio::test]
async fn attached_updated_detached_and_stale_events_converge_on_latest_provider_state() {
    let function = required(aex_live_finance_ingest::FUNCTION_ENV);
    let api = required(aex_live_finance_ingest::API_URL_ENV);
    let bearer = required(aex_live_finance_ingest::API_BEARER_ENV);
    let attached = request(aex_live_finance_ingest::ATTACHED_ENV);
    let updated = request(aex_live_finance_ingest::UPDATED_ENV);
    let detached = request(aex_live_finance_ingest::DETACHED_ENV);
    let stale = request(aex_live_finance_ingest::STALE_ATTACHED_ENV);
    let missing_owner = request(aex_live_finance_ingest::MISSING_OWNER_ENV);

    assert!(
        stale["event"]["occurredAt"]
            .as_str()
            .expect("stale occurredAt fixture")
            < detached["event"]["occurredAt"]
                .as_str()
                .expect("detach occurredAt fixture"),
        "the stale fixture must predate detach"
    );
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let lambda = aws_sdk_lambda::Client::new(&aws);
    let http = reqwest::Client::new();

    invoke(&lambda, &function, &missing_owner, "quarantined").await;
    invoke(&lambda, &function, &attached, "applied").await;
    let attached_display = display(&attached);
    let page = cards(&http, &api, &bearer).await;
    let card = page
        .items
        .iter()
        .find(|card| card.last4 == attached_display.0)
        .expect("attached card is immediately readable");
    assert_eq!(u64::from(card.expiry_month), attached_display.1);
    assert_eq!(u64::from(card.expiry_year), attached_display.2);
    let id = card.id.clone();

    invoke(&lambda, &function, &updated, "applied").await;
    let updated_display = display(&updated);
    let page = cards(&http, &api, &bearer).await;
    let card = page
        .items
        .iter()
        .find(|card| card.id == id)
        .expect("update preserves the Aex payment-method identity");
    assert_eq!(card.last4, updated_display.0);
    assert_eq!(u64::from(card.expiry_month), updated_display.1);
    assert_eq!(u64::from(card.expiry_year), updated_display.2);

    invoke(&lambda, &function, &detached, "applied").await;
    invoke(&lambda, &function, &stale, "applied").await;
    let page = cards(&http, &api, &bearer).await;
    assert!(
        page.items.iter().all(|card| card.id != id),
        "detach remains terminal against a late stale attach"
    );
}
