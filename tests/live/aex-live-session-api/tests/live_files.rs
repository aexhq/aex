//! Deployed direct-file journey: durable registry materialization, exact-generation
//! live multipart transfer, native resume, manual lifecycle, and termination loss.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_test_harness::{
    CleanupLedger, Entry, Lane, ReclaimError, Reclaimer, ResourceKind, Terminal, TestCaseId,
    TestRun,
};
use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};
use reqwest::header::{CONTENT_TYPE, ETAG, HeaderName, HeaderValue};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const OWNER: &str = "regional-services";
const CASE: &str = "session-live-files";
const LOGICAL_PART_BYTES: usize = 4_194_304;
const LIVE_BUDGET_MICRO_USD: u64 = 50_000;
const ESTIMATED_RUN_MICRO_USD: u64 = 25_000;
static IDS: AtomicU64 = AtomicU64::new(0);

struct PublicReclaimer {
    base: String,
    token: String,
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl Reclaimer for PublicReclaimer {
    fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError> {
        let outcome = tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                match entry.kind {
                    ResourceKind::S3Object => {
                        let response = self
                            .client
                            .delete(format!(
                                "{}/api/workspace/files/{}",
                                self.base, entry.identity
                            ))
                            .bearer_auth(&self.token)
                            .send()
                            .await
                            .map_err(|error| error.to_string())?;
                        (response.status() == StatusCode::NO_CONTENT)
                            .then_some(())
                            .ok_or_else(|| {
                                format!("registry delete answered {}", response.status())
                            })
                    }
                    ResourceKind::Session => {
                        let operation = operation_id("cleanup-delete");
                        let response = self
                            .client
                            .post(format!(
                                "{}/api/sessions/{}/deletions",
                                self.base, entry.identity
                            ))
                            .bearer_auth(&self.token)
                            .header("Aex-Operation-Id", operation.to_string())
                            .json(&json!({}))
                            .send()
                            .await
                            .map_err(|error| error.to_string())?;
                        if response.status() != StatusCode::ACCEPTED {
                            return Err(format!("session delete answered {}", response.status()));
                        }
                        let operation = operation.to_string();
                        wait_operation(&self.client, &self.base, &self.token, &operation).await
                    }
                    _ => Err(format!("no public cleanup for {}", entry.kind)),
                }
            })
        });
        outcome.map_err(|reason| ReclaimError::new(entry, reason))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registered_bytes_survive_suspend_and_live_bytes_die_at_termination() {
    let base = aex_test_harness::required_env!("AEX_API_URL");
    let token = aex_test_harness::required_env!("AEX_API_KEY");
    let provider = aex_test_harness::required_env!("AEX_LIVE_PROVIDER");
    let model = aex_test_harness::required_env!("AEX_LIVE_MODEL");
    let credential = aex_test_harness::required_env!("AEX_LIVE_PROVIDER_CREDENTIAL_ID");
    let base = base.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1_100))
        .build()
        .expect("the bounded live client must build");
    let run = TestRun::mint(Lane::E2e, OWNER, LIVE_BUDGET_MICRO_USD);
    assert!(run.ledger().install_reclaimer(Arc::new(PublicReclaimer {
        base: base.to_owned(),
        token: token.clone(),
        client: client.clone(),
        runtime: tokio::runtime::Handle::current(),
    })));
    assert!(
        run.budget().charge(ESTIMATED_RUN_MICRO_USD).may_continue(),
        "the fixed five-minute generation estimate must fit the live budget"
    );

    let registered_name = run.resource_name("session-file");
    run.ledger().record(Entry::new(
        ResourceKind::S3Object,
        registered_name.clone(),
        Terminal::Deleted,
        TestCaseId(CASE.to_owned()),
    ));
    let seed = fixture_bytes(5 * 1024 * 1024 + 257, 17);
    let journey = async {
        register_file(&client, base, &token, &registered_name, "/seed.bin", &seed).await?;
        let session = create_session(
            &client,
            base,
            &token,
            &provider,
            &model,
            &credential,
            &registered_name,
        )
        .await?;
        run.ledger().record(Entry::new(
            ResourceKind::Session,
            session.clone(),
            Terminal::Deleted,
            TestCaseId(CASE.to_owned()),
        ));

        let (materialized, _) = download_live(&client, base, &token, &session, "/seed.bin").await?;
        require(
            materialized == seed,
            "create returned before registered bytes were materialized",
        )?;

        command(&client, base, &token, &session, "suspensions").await?;
        let (after_native_resume, resumed) =
            download_live(&client, base, &token, &session, "/seed.bin").await?;
        require(
            resumed,
            "live download did not report native same-generation resume",
        )?;
        require(
            after_native_resume == seed,
            "suspension changed materialized bytes",
        )?;

        let uploaded = fixture_bytes(LOGICAL_PART_BYTES + 4_097, 91);
        upload_live(&client, base, &token, &session, "/roundtrip.bin", &uploaded).await?;
        let (downloaded, _) =
            download_live(&client, base, &token, &session, "/roundtrip.bin").await?;
        require(
            downloaded == uploaded,
            "live multipart round trip changed bytes",
        )?;

        command(&client, base, &token, &session, "suspensions").await?;
        command(&client, base, &token, &session, "resumptions").await?;
        command(&client, base, &token, &session, "terminations").await?;

        let terminal = client
            .post(format!("{base}/api/sessions/{session}/files/live/stat"))
            .bearer_auth(&token)
            .json(&json!({ "path": "/roundtrip.bin" }))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = terminal.status();
        let body: Value = terminal.json().await.map_err(|error| error.to_string())?;
        require(
            status == StatusCode::CONFLICT,
            "terminated live stat did not return 409",
        )?;
        require(
            body.pointer("/error/code").and_then(Value::as_str) == Some("session_terminated"),
            "terminated live stat did not fail at the durable lifecycle guard",
        )?;
        Ok::<(), String>(())
    }
    .await;

    let cleanup = run.ledger().reclaim_all();
    let cleanup_failure = (!cleanup.is_complete()).then(|| {
        cleanup
            .failed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    });
    write_hygiene(&run);
    if let Some(failure) = cleanup_failure {
        panic!("live-file cleanup failed: {failure}");
    }
    journey.unwrap_or_else(|error| panic!("live-file journey failed: {error}"));
}

async fn register_file(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    name: &str,
    mount_path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let whole = digest(bytes);
    let upload = json_response(
        client
            .post(format!("{base}/api/workspace/uploads"))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("registry-open"))
            .json(&json!({
                "sizeBytes": bytes.len().to_string(),
                "sha256": whole,
                "contentType": "application/octet-stream"
            })),
        StatusCode::CREATED,
        "registry upload create",
    )
    .await?;
    let upload_id = text(&upload, "/id")?;
    let part_size = decimal(&upload, "/partSizeBytes")?;
    let parts = bytes
        .chunks(part_size)
        .enumerate()
        .map(|(index, part)| {
            json!({
                "partNumber": index + 1,
                "sha256": digest(part),
                "sizeBytes": part.len().to_string()
            })
        })
        .collect::<Vec<_>>();
    let grants = json_response(
        client
            .post(format!("{base}/api/workspace/uploads/{upload_id}/parts"))
            .bearer_auth(token)
            .json(&json!({ "parts": parts })),
        StatusCode::OK,
        "registry upload grants",
    )
    .await?;
    let mut receipts = Vec::new();
    for grant in grants
        .pointer("/grants")
        .and_then(Value::as_array)
        .ok_or_else(|| "upload grants omitted grants".to_owned())?
    {
        let number = usize::try_from(
            grant
                .get("partNumber")
                .and_then(Value::as_u64)
                .ok_or_else(|| "grant omitted partNumber".to_owned())?,
        )
        .map_err(|_| "grant partNumber exceeds this client".to_owned())?;
        let part = bytes
            .chunks(part_size)
            .nth(number - 1)
            .ok_or_else(|| "grant selected an unknown part".to_owned())?;
        let mut request = client.put(text(grant, "/url")?).body(part.to_vec());
        for header in grant
            .get("headers")
            .and_then(Value::as_array)
            .ok_or_else(|| "grant omitted signed headers".to_owned())?
        {
            let name = HeaderName::from_bytes(text(header, "/name")?.as_bytes())
                .map_err(|error| error.to_string())?;
            let value = HeaderValue::from_str(text(header, "/value")?)
                .map_err(|error| error.to_string())?;
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        require(
            response.status().is_success(),
            "presigned multipart PUT failed",
        )?;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| "multipart PUT omitted ETag".to_owned())?;
        receipts.push(json!({ "partNumber": number, "etag": etag }));
    }
    json_response(
        client
            .post(format!(
                "{base}/api/workspace/uploads/{upload_id}/completion"
            ))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("registry-complete"))
            .json(&json!({ "parts": receipts })),
        StatusCode::OK,
        "registry upload complete",
    )
    .await?;
    json_response(
        client
            .put(format!("{base}/api/workspace/files/{name}"))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("registry-file"))
            .json(&json!({
                "mountPath": mount_path,
                "content": {
                    "type": "upload",
                    "uploadId": upload_id,
                    "sha256": whole,
                    "sizeBytes": bytes.len().to_string()
                },
                "mediaType": "application/octet-stream",
                "mode": "0644"
            })),
        StatusCode::OK,
        "registry file put",
    )
    .await?;
    Ok(())
}

async fn create_session(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    provider: &str,
    model: &str,
    credential: &str,
    file: &str,
) -> Result<String, String> {
    let session = json_response(
        client
            .post(format!("{base}/api/sessions"))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("session-create"))
            .json(&json!({
                "provider": provider,
                "model": model,
                "providerCredentialId": credential,
                "registered": { "files": [file] },
                "compute": { "size": "512mb" },
                "network": { "hands": { "mode": "none" } }
            })),
        StatusCode::CREATED,
        "session create and materialization",
    )
    .await?;
    require(
        session.get("status").and_then(Value::as_str) == Some("idle"),
        "session create returned before idle readiness",
    )?;
    Ok(text(&session, "/id")?.to_owned())
}

async fn upload_live(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: &str,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let upload = json_response(
        client
            .post(format!("{base}/api/sessions/{session}/files/live/uploads"))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("live-upload-open"))
            .json(&json!({
                "path": path,
                "sizeBytes": bytes.len().to_string(),
                "sha256": digest(bytes),
                "mode": "0644"
            })),
        StatusCode::CREATED,
        "live upload create",
    )
    .await?;
    let id = text(&upload, "/id")?;
    for (index, part) in bytes.chunks(LOGICAL_PART_BYTES).enumerate() {
        json_response(
            client
                .put(format!(
                    "{base}/api/sessions/{session}/files/live/uploads/{id}/parts/{}?sha256={}",
                    index + 1,
                    digest(part)
                ))
                .bearer_auth(token)
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(part.to_vec()),
            StatusCode::OK,
            "live upload part",
        )
        .await?;
    }
    let complete = json_response(
        client
            .post(format!(
                "{base}/api/sessions/{session}/files/live/uploads/{id}/completion"
            ))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("live-upload-complete"))
            .json(&json!({})),
        StatusCode::OK,
        "live upload complete",
    )
    .await?;
    require(
        complete.get("state").and_then(Value::as_str) == Some("complete"),
        "live upload did not publish atomically",
    )
}

async fn download_live(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: &str,
    path: &str,
) -> Result<(Vec<u8>, bool), String> {
    let opened = json_response(
        client
            .post(format!(
                "{base}/api/sessions/{session}/files/live/downloads"
            ))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("live-download-open"))
            .json(&json!({ "path": path })),
        StatusCode::CREATED,
        "live download create",
    )
    .await?;
    let id = text(&opened, "/id")?;
    let version = text(&opened, "/version")?;
    let mut bytes = Vec::new();
    for part in opened
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(|| "live download omitted parts".to_owned())?
    {
        let number = part
            .get("partNumber")
            .and_then(Value::as_u64)
            .ok_or_else(|| "download part omitted partNumber".to_owned())?;
        let response = client
            .get(format!(
                "{base}/api/sessions/{session}/files/live/downloads/{id}/parts/{number}?version={version}"
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        require(
            response.status() == StatusCode::OK,
            "live download part failed",
        )?;
        let part_bytes = response.bytes().await.map_err(|error| error.to_string())?;
        require(
            digest(&part_bytes) == text(part, "/sha256")?,
            "live download part digest changed",
        )?;
        bytes.extend_from_slice(&part_bytes);
    }
    let whole = digest(&bytes);
    require(
        whole == text(&opened, "/sha256")?,
        "whole live download digest changed",
    )?;
    let complete = json_response(
        client
            .post(format!(
                "{base}/api/sessions/{session}/files/live/downloads/{id}/completion"
            ))
            .bearer_auth(token)
            .header("Idempotency-Key", replay("live-download-complete"))
            .json(&json!({ "version": version, "sha256": whole })),
        StatusCode::OK,
        "live download complete",
    )
    .await?;
    require(
        complete.get("state").and_then(Value::as_str) == Some("verified"),
        "live download did not finish exact-version verification",
    )?;
    let resumed = opened
        .pointer("/workspaceAccess/resumed")
        .and_then(Value::as_bool)
        .ok_or_else(|| "download omitted workspaceAccess.resumed".to_owned())?;
    Ok((bytes, resumed))
}

async fn command(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: &str,
    path: &str,
) -> Result<(), String> {
    let operation = operation_id(path);
    json_response(
        client
            .post(format!("{base}/api/sessions/{session}/{path}"))
            .bearer_auth(token)
            .header("Aex-Operation-Id", operation.to_string())
            .json(&json!({})),
        StatusCode::ACCEPTED,
        path,
    )
    .await?;
    wait_operation(client, base, token, &operation.to_string()).await
}

async fn wait_operation(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    operation: &str,
) -> Result<(), String> {
    for _ in 0..360 {
        let response = client
            .get(format!("{base}/api/operations/{operation}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status() != StatusCode::OK {
            return Err(format!("operation read answered {}", response.status()));
        }
        let body: Value = response.json().await.map_err(|error| error.to_string())?;
        match body.get("status").and_then(Value::as_str) {
            Some("succeeded") => return Ok(()),
            Some("failed" | "cancelled") => return Err(format!("operation {operation} failed")),
            Some("queued" | "running") => tokio::time::sleep(Duration::from_millis(500)).await,
            _ => return Err(format!("operation {operation} returned an unknown state")),
        }
    }
    Err(format!(
        "operation {operation} exceeded the 180-second wait"
    ))
}

async fn json_response(
    request: RequestBuilder,
    expected: StatusCode,
    label: &str,
) -> Result<Value, String> {
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    if status != expected {
        let request_id = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|body| {
                body.pointer("/error/requestId")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "<missing>".to_owned());
        return Err(format!(
            "{label} answered {}, request_id={request_id}",
            status.as_u16()
        ));
    }
    serde_json::from_slice(&body).map_err(|_| format!("{label} returned malformed JSON"))
}

fn text<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("response omitted {pointer}"))
}

fn decimal(value: &Value, pointer: &str) -> Result<usize, String> {
    text(value, pointer)?
        .parse()
        .map_err(|_| format!("{pointer} is not a bounded decimal"))
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn fixture_bytes(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|index| {
            u8::try_from(index % 256)
                .expect("a reduced fixture position is one byte")
                .wrapping_mul(31)
                .wrapping_add(seed)
        })
        .collect()
}

fn replay(label: &str) -> String {
    format!("{label}-{}", IDS.fetch_add(1, Ordering::Relaxed))
}

fn operation_id(label: &str) -> OperationId {
    let millis = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the live clock must be after the epoch")
            .as_millis(),
    )
    .expect("the live clock must fit the UUIDv7 timestamp");
    let mut digest = Sha256::new();
    digest.update(label.as_bytes());
    digest.update(IDS.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    let bytes = digest.finalize();
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&bytes[..10]);
    OperationId::from_uuid7(Uuid7::compose(millis, entropy))
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.to_owned())
}

fn write_hygiene(run: &TestRun) {
    use std::fmt::Write as _;

    let ledger = run.ledger();
    ledger.flush().expect("the cleanup ledger must flush");
    let provisioned = ledger
        .entries()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let canonical = serde_json::to_vec(&provisioned).expect("ledger names serialize");
    let digest = Sha256::digest(canonical)
        .iter()
        .fold("sha256:".to_owned(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        });
    let residue = ledger
        .take_residue_report()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let evidence = json!({
        "schema": "aex.release-evidence-hygiene.v1",
        "suite": "e2e",
        "releaseId": aex_test_harness::required_env!("RELEASE_ID"),
        "workflowRunId": aex_test_harness::required_env!("GITHUB_RUN_ID"),
        "budgetMicroUsd": run.budget().soft_micro_usd(),
        "spentMicroUsd": run.budget().spent_micro_usd(),
        "cleanupLedgerDigest": digest,
        "provisioned": provisioned,
        "residue": residue,
        "secretCanaryDigest": aex_test_harness::required_env!("SECRET_CANARY_DIGEST"),
        "secretCanaryObserved": false
    });
    fs::write(
        aex_test_harness::required_env!("AEX_RELEASE_EVIDENCE_HYGIENE_PATH"),
        serde_json::to_vec(&evidence).expect("hygiene evidence serializes"),
    )
    .expect("hygiene evidence must be written");
}
