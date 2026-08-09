//! Exact deployed-plane admission check selected only by the release evidence lane.
//!
//! # The cleanup ledger
//!
//! `[role.live_companion]` in `release/policy/test-profiles.toml` says a live
//! companion "owns the cleanup-ledger flush", and this is the reference for what
//! that means. The hygiene receipt this file writes is an input to the release
//! gate, so every field of it that describes cleanup is **derived from a real
//! [`CleanupLedger`]**, never written down:
//!
//! * `cleanupLedgerDigest` is the digest of the ledger's own entries;
//! * `provisioned` is what the run recorded;
//! * `residue` is what the run recorded and could not release.
//!
//! Those three were previously constants — the digest was `sha256("[]")` with
//! two empty arrays beside it — which meant the receipt asserted a clean run
//! rather than reporting one. A lane that reports success having deleted nothing
//! is the exact defect `aex_test_harness::ledger` exists to stop, and a constant
//! is that defect in its purest form: it would have kept reading `residue: []`
//! however much this case went on to leak.
//!
//! The reclamation path is real. [`RegistryFileReclaimer`] issues the public
//! contract's `registry_files_delete` (`DELETE /api/workspace/files/{name}`), so
//! `release` only stamps an entry once the plane actually gave the resource up.
//! There is deliberately no second "the test deleted it itself" route.

use std::fs;
use std::sync::Arc;

use aex_test_harness::{
    CleanupLedger, Entry, Lane, ReclaimError, Reclaimer, ResourceKind, Terminal, TestCaseId,
    TestRun,
};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// The owning stream, as `[package.metadata.aex] owner` declares it.
const OWNER: &str = "regional-services";

/// Reclaims a registered workspace file through the public contract.
///
/// # Why `404` is a failure here and not "already gone"
///
/// The trait asks implementations to be idempotent, because a sweep by tag and
/// this ledger can race, and the usual way to honour that is to read `404` as
/// success. That reading is unsafe *today*: the delivery graph defers
/// `registry_files_delete` ("Registry mutation and content-lifecycle composition
/// is incomplete"), so an unmounted route answers `404` for exactly the same
/// bytes an already-deleted resource does. Treating them alike would stamp
/// `released_at` on a resource that is still there and publish a receipt saying
/// the run was clean — the silent success this whole module exists to stop.
///
/// So `404` fails, loudly and by name. The cost is a false residue report if the
/// janitor genuinely won a race, which is loud and recoverable; the cost the
/// other way is a false clean, which is neither. When the route is mounted this
/// can be relaxed with evidence, and [`is_reclaimed`] is the one place to change.
struct RegistryFileReclaimer {
    base: String,
    token: String,
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

/// Whether an answer to `DELETE /api/workspace/files/{name}` proves the resource
/// is gone. Split out so the rule is testable without a plane.
fn is_reclaimed(status: StatusCode) -> bool {
    status.is_success()
}

impl Reclaimer for RegistryFileReclaimer {
    fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError> {
        let url = format!("{}/api/workspace/files/{}", self.base, entry.identity);
        let request = self.client.delete(&url).bearer_auth(&self.token).send();
        // `Reclaimer` is synchronous because `Drop` is, so the async delete is
        // driven on the runtime this test is already running under.
        let response = tokio::task::block_in_place(|| self.runtime.block_on(request))
            .map_err(|error| ReclaimError::new(entry, format!("DELETE {url} failed: {error}")))?;
        let status = response.status();
        if is_reclaimed(status) {
            Ok(())
        } else if status == StatusCode::NOT_FOUND {
            Err(ReclaimError::new(
                entry,
                format!(
                    "DELETE {url} answered 404, which is ambiguous while `registry_files_delete` \
                     is a deferred route: the resource may be gone, or the route may not be \
                     mounted. Refusing to mark it released on evidence that cannot tell those \
                     apart."
                ),
            ))
        } else {
            Err(ReclaimError::new(
                entry,
                format!("DELETE {url} answered {}", status.as_u16()),
            ))
        }
    }
}

/// Mints the run and installs the one reclamation path it may ever use.
///
/// Installed once and never replaced: a lane that could swap the reclaimer
/// mid-run could swap in one that deletes nothing.
fn mint_run_with_reclamation_path(base: &str, token: &str, client: &reqwest::Client) -> TestRun {
    let run = TestRun::mint(Lane::E2e, OWNER, 0);
    let installed = run
        .ledger()
        .install_reclaimer(Arc::new(RegistryFileReclaimer {
            base: base.to_owned(),
            token: token.to_owned(),
            client: client.clone(),
            runtime: tokio::runtime::Handle::current(),
        }));
    assert!(
        installed,
        "the reclamation path must be installed exactly once; a ledger that could swap it \
         mid-run could swap in one that deletes nothing"
    );
    run
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_registry_inventory_is_admitted_and_anonymous_access_is_denied() {
    let base = aex_test_harness::required_env!("AEX_API_URL");
    let token = aex_test_harness::required_env!("AEX_API_KEY");
    let base = base.trim_end_matches('/');
    let inventory_url = format!("{base}/api/workspace/files?limit=1");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("the bounded live client must build");

    // Minted before the first request so anything this case goes on to create
    // has a run identity, a tag set and a ledger waiting for it.
    let run = mint_run_with_reclamation_path(base, &token, &client);

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

    // Everything still outstanding goes through the installed reclaimer, in
    // dependency order. Nothing here creates yet, so this is a no-op today and a
    // real reclamation the moment a case does — which is the point: the path is
    // exercised by construction rather than added later beside a create.
    let summary = run.ledger().reclaim_all();
    assert!(
        summary.is_complete(),
        "run {} could not reclaim {} resource(s): {}",
        run.id(),
        summary.failed.len(),
        summary
            .failed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );

    write_hygiene(run.ledger());
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

/// The canonical rendering of the ledger the digest is taken over.
///
/// One function, used both to compute the digest and to fill `provisioned`, so
/// the receipt cannot claim a digest of one thing and a list of another.
fn provisioned_names(ledger: &CleanupLedger) -> Vec<String> {
    ledger
        .entries()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
}

/// `sha256:` over the JSON array of provisioned names.
fn ledger_digest(names: &[String]) -> String {
    use std::fmt::Write as _;

    let canonical = serde_json::to_vec(names).expect("the ledger rendering must serialize");
    let digest = Sha256::digest(&canonical);
    digest.iter().fold("sha256:".to_owned(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Writes the hygiene receipt from the ledger, not from constants.
///
/// [`CleanupLedger::take_residue_report`] is the one way past the ledger's loud
/// `Drop`, and it exists precisely for this: the release lane must carry residue
/// into the receipt rather than abort the process holding it. Taking the report
/// does not make residue acceptable — the receipt gate refuses a lane that
/// carries any, and the janitor still finds it by tag.
fn write_hygiene(ledger: &CleanupLedger) {
    let path = aex_test_harness::required_env!("AEX_RELEASE_EVIDENCE_HYGIENE_PATH");
    let release_id = aex_test_harness::required_env!("RELEASE_ID");
    let workflow_run_id = aex_test_harness::required_env!("GITHUB_RUN_ID");
    let secret_canary_digest = aex_test_harness::required_env!("SECRET_CANARY_DIGEST");

    ledger
        .flush()
        .expect("the cleanup ledger must be written before the receipt that digests it");

    let provisioned = provisioned_names(ledger);
    let residue: Vec<String> = ledger
        .take_residue_report()
        .iter()
        .map(ToString::to_string)
        .collect();

    let evidence = json!({
        "schema": "aex.release-evidence-hygiene.v1",
        "suite": "e2e",
        "releaseId": release_id,
        "workflowRunId": workflow_run_id,
        "budgetMicroUsd": 0,
        "spentMicroUsd": 0,
        "cleanupLedgerDigest": ledger_digest(&provisioned),
        "provisioned": provisioned,
        "residue": residue,
        "secretCanaryDigest": secret_canary_digest,
        "secretCanaryObserved": false
    });
    let bytes = serde_json::to_vec(&evidence).expect("the hygiene evidence must serialize");
    fs::write(path, bytes).expect("the hygiene evidence must be written");
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

    /// A reclaimer that records what it was asked to delete and answers as told,
    /// so the ledger's "released means released" rule can be observed here
    /// without a plane.
    struct ScriptedReclaimer {
        deleted: std::sync::Mutex<Vec<String>>,
        outcome: Result<(), String>,
    }

    impl Reclaimer for ScriptedReclaimer {
        fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError> {
            self.deleted
                .lock()
                .expect("the scripted reclaimer is not poisoned")
                .push(entry.identity.clone());
            self.outcome
                .clone()
                .map_err(|reason| ReclaimError::new(entry, reason))
        }
    }

    fn ledger_with(outcome: Result<(), String>) -> (Arc<ScriptedReclaimer>, TestRun) {
        let run = TestRun::mint(Lane::E2e, OWNER, 0);
        let reclaimer = Arc::new(ScriptedReclaimer {
            deleted: std::sync::Mutex::new(Vec::new()),
            outcome,
        });
        assert!(run.ledger().install_reclaimer(reclaimer.clone()));
        (reclaimer, run)
    }

    fn record_one(run: &TestRun, identity: &str) {
        run.ledger().record(Entry::new(
            ResourceKind::S3Object,
            identity,
            Terminal::Deleted,
            TestCaseId("registry-inventory".to_owned()),
        ));
    }

    /// The regression the constants were: the digest and the two arrays have to
    /// move when the run creates something. `sha256("[]")` could not.
    #[test]
    fn the_hygiene_digest_is_derived_from_the_ledger_rather_than_written_down() {
        let (_reclaimer, run) = ledger_with(Ok(()));
        let empty = provisioned_names(run.ledger());
        let empty_digest = ledger_digest(&empty);

        record_one(&run, &run.resource_name("registered-file"));

        let after = provisioned_names(run.ledger());
        assert_eq!(after.len(), 1, "the entry must reach the receipt's list");
        assert_ne!(
            ledger_digest(&after),
            empty_digest,
            "a receipt whose cleanup digest cannot move is a constant, not evidence"
        );
        assert!(
            after[0].contains(run.id().as_str()),
            "the provisioned name must carry the run id that joins it to the janitor's tags"
        );

        run.ledger().reclaim_all();
        let _ = run.ledger().take_residue_report();
    }

    /// `release` stamps only after the plane gave the resource up, so a failed
    /// delete has to leave the entry as residue and reach the receipt.
    #[test]
    fn a_reclamation_the_plane_refused_reaches_the_receipt_as_residue() {
        let (reclaimer, run) = ledger_with(Err("DELETE answered 409".to_owned()));
        let identity = run.resource_name("stuck-file");
        record_one(&run, &identity);

        let summary = run.ledger().reclaim_all();
        assert!(!summary.is_complete(), "the scripted refusal must be seen");
        assert_eq!(
            reclaimer
                .deleted
                .lock()
                .expect("the scripted reclaimer is not poisoned")
                .len(),
            1,
            "the reclamation must actually have been attempted"
        );

        let residue: Vec<String> = run
            .ledger()
            .take_residue_report()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(residue.len(), 1);
        assert!(residue[0].contains(&identity));
    }

    /// Only an answer that proves the resource is gone may release an entry.
    #[test]
    fn only_a_successful_delete_counts_as_reclaimed() {
        assert!(is_reclaimed(StatusCode::NO_CONTENT));
        assert!(is_reclaimed(StatusCode::OK));
        // 404 is the answer an unmounted deferred route gives as well as a
        // deleted resource, so it may not release anything.
        assert!(!is_reclaimed(StatusCode::NOT_FOUND));
        assert!(!is_reclaimed(StatusCode::CONFLICT));
        assert!(!is_reclaimed(StatusCode::FORBIDDEN));
        assert!(!is_reclaimed(StatusCode::INTERNAL_SERVER_ERROR));
    }

    /// The receipt digests exactly the list it publishes.
    #[test]
    fn the_digest_covers_the_list_the_receipt_carries() {
        let (_reclaimer, run) = ledger_with(Ok(()));
        record_one(&run, &run.resource_name("a"));
        record_one(&run, &run.resource_name("b"));

        let provisioned = provisioned_names(run.ledger());
        let expected = ledger_digest(&provisioned);
        assert_eq!(ledger_digest(&provisioned_names(run.ledger())), expected);
        assert_ne!(expected, ledger_digest(&provisioned[..1]));

        run.ledger().reclaim_all();
        let _ = run.ledger().take_residue_report();
    }
}
