//! Exact deployed-plane admission check selected only by the release evidence lane.

use std::fs;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

#[tokio::test]
async fn authenticated_registry_inventory_is_admitted_and_anonymous_access_is_denied() {
    let base = aex_test_harness::required_env!("AEX_API_URL");
    let token = aex_test_harness::required_env!("AEX_API_KEY");
    let url = format!("{}/api/workspace/files?limit=1", base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("the bounded live client must build");

    let anonymous = client
        .get(&url)
        .send()
        .await
        .expect("the anonymous admission check must receive a response");
    assert!(
        matches!(
            anonymous.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "anonymous registry inventory returned {}",
        anonymous.status()
    );

    let admitted = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .expect("the authenticated admission check must receive a response");
    assert_eq!(admitted.status(), StatusCode::OK);
    let body: Value = admitted
        .json()
        .await
        .expect("the authenticated registry inventory must be JSON");
    assert!(
        body.get("items").is_some_and(Value::is_array),
        "the authenticated registry inventory omitted its items array"
    );

    write_hygiene();
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
