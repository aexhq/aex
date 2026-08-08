//! Exact deployed-plane admission check selected only by the release evidence lane.

use std::fs;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

#[tokio::test]
async fn authenticated_registry_inventory_is_admitted_and_anonymous_access_is_denied() {
    let base = aex_test_harness::required_env!("AEX_API_URL");
    let token = aex_test_harness::required_env!("AEX_API_KEY");
    let base = base.trim_end_matches('/');
    let inventory_url = format!("{base}/api/workspace/files?limit=1");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("the bounded live client must build");

    // The public contract's regional `registry_files_list` route is the token
    // preflight: GET /api/workspace/files?limit=1 -> RegisteredFilePage. This
    // must be the first network request so a stale bearer fails clearly.
    let preflight = client
        .get(&inventory_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("the authenticated inventory preflight must receive a response");
    let (preflight_status, preflight_headers, preflight_body) =
        collect_response(preflight, "the authenticated inventory preflight").await;
    let preflight_diagnostic =
        redacted_response_diagnostic(preflight_status, &preflight_headers, &preflight_body);
    assert_eq!(
        preflight_status,
        StatusCode::OK,
        "bearer token preflight GET /api/workspace/files?limit=1 failed ({preflight_diagnostic})"
    );
    assert_json_content_type(
        &preflight_headers,
        "the authenticated inventory preflight",
        &preflight_diagnostic,
    );
    let preflight_page: Value = parse_json(
        &preflight_body,
        "the authenticated inventory preflight",
        &preflight_diagnostic,
    );
    assert_inventory_page(
        &preflight_page,
        "the authenticated inventory preflight",
        &preflight_diagnostic,
    );

    let anonymous = client
        .get(&inventory_url)
        .send()
        .await
        .expect("the anonymous admission check must receive a response");
    let (anonymous_status, anonymous_headers, anonymous_body) =
        collect_response(anonymous, "the anonymous admission check").await;
    let anonymous_diagnostic =
        redacted_response_diagnostic(anonymous_status, &anonymous_headers, &anonymous_body);
    assert!(
        matches!(
            anonymous_status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "anonymous registry inventory was not unauthorized ({anonymous_diagnostic})"
    );

    let admitted = client
        .get(&inventory_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("the authenticated admission check must receive a response");
    let (admitted_status, admitted_headers, admitted_body) =
        collect_response(admitted, "the authenticated registry inventory").await;
    let admitted_diagnostic =
        redacted_response_diagnostic(admitted_status, &admitted_headers, &admitted_body);
    assert_eq!(
        admitted_status,
        StatusCode::OK,
        "authenticated registry inventory was not admitted ({admitted_diagnostic})"
    );
    assert_json_content_type(
        &admitted_headers,
        "the authenticated registry inventory",
        &admitted_diagnostic,
    );
    let body: Value = parse_json(
        &admitted_body,
        "the authenticated registry inventory",
        &admitted_diagnostic,
    );
    assert_inventory_page(
        &body,
        "the authenticated registry inventory",
        &admitted_diagnostic,
    );

    write_hygiene();
}

async fn collect_response(
    response: reqwest::Response,
    label: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .unwrap_or_else(|_| panic!("{label} response body could not be read"));
    (status, headers, body.to_vec())
}

fn parse_json(body: &[u8], label: &str, diagnostic: &str) -> Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| panic!("{label} did not return JSON ({diagnostic})"))
}

fn assert_json_content_type(headers: &HeaderMap, label: &str, diagnostic: &str) {
    let is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    assert!(
        is_json,
        "{label} did not return application/json ({diagnostic})"
    );
}

fn assert_inventory_page(body: &Value, label: &str, diagnostic: &str) {
    assert!(
        body.get("items").is_some_and(Value::is_array),
        "{label} omitted its items array ({diagnostic})"
    );
}

fn redacted_response_diagnostic(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> String {
    let content_type =
        safe_header(headers, CONTENT_TYPE.as_str()).unwrap_or_else(|| "<missing>".to_owned());
    let request_id = safe_header(headers, "x-request-id")
        .or_else(|| error_request_id(body))
        .unwrap_or_else(|| "<missing>".to_owned());
    format!(
        "status={}, content_type={content_type}, request_id={request_id}",
        status.as_u16()
    )
}

fn safe_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(sanitize_diagnostic_value)
}

fn error_request_id(body: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(body).ok()?;
    body.get("error")?
        .get("requestId")?
        .as_str()
        .map(sanitize_diagnostic_value)
}

fn sanitize_diagnostic_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_include_trace_metadata_without_rendering_error_body() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let body = br#"{"error":{"message":"sensitive-body-text","requestId":"req-test-0001"}}"#;

        let diagnostic = redacted_response_diagnostic(StatusCode::UNAUTHORIZED, &headers, body);

        assert_eq!(
            diagnostic,
            "status=401, content_type=application/json, request_id=req-test-0001"
        );
        assert!(!diagnostic.contains("sensitive-body-text"));
    }

    #[test]
    fn inventory_preflight_checks_the_published_inventory_page_shape() {
        let inventory = json!({
            "items": []
        });

        assert_inventory_page(&inventory, "test inventory", "test diagnostic");
    }
}

fn write_hygiene() {
    let path = aex_test_harness::required_env!("AEX_RELEASE_EVIDENCE_HYGIENE_PATH");
    let release_id = aex_test_harness::required_env!("RELEASE_ID");
    let workflow_run_id = aex_test_harness::required_env!("GITHUB_RUN_ID");
    let secret_canary_digest = aex_test_harness::required_env!("SECRET_CANARY_DIGEST");
    let digest = Sha256::digest(b"[]");
    let cleanup_digest = format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let evidence = json!({
        "schema": "aex.release-evidence-hygiene.v1",
        "suite": "e2e",
        "releaseId": release_id,
        "workflowRunId": workflow_run_id,
        "budgetMicroUsd": 0,
        "spentMicroUsd": 0,
        "cleanupLedgerDigest": cleanup_digest,
        "provisioned": [],
        "residue": [],
        "secretCanaryDigest": secret_canary_digest,
        "secretCanaryObserved": false
    });
    let bytes = serde_json::to_vec(&evidence).expect("the hygiene evidence must serialize");
    fs::write(path, bytes).expect("the hygiene evidence must be written");
}
