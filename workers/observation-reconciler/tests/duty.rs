//! Unit evidence for the reconciler's configuration, capability grant and duty
//! engine.
//!
//! The engine's request shapes are asserted on the **serialized** body captured
//! from a real client, because that is what `DynamoDB` evaluates. Nothing here
//! reaches AWS: the transport is a capturing double.

use std::collections::{BTreeMap, HashMap};

use aex_observation_domain::keys::{self, ControlDomain};
use aex_observation_domain::limits;
use aex_observation_store_dynamodb::composition::{Capability, Role};
use aex_observation_store_dynamodb::expressions::Index;
use aex_observation_store_dynamodb::health::{HEALTHZ, READYZ};
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;
use aws_smithy_http_client::test_util::{
    CaptureRequestReceiver, ReplayEvent, StaticReplayClient, capture_request,
};
use aws_smithy_runtime_api::client::http::HttpClient;
use aws_smithy_types::body::SdkBody;

use observation_reconciler::config::{
    Config, DUTY_SHARDS_VAR, DUTY_VAR, MAX_ATTEMPTS_VAR, OBSERVATION_BUCKET_VAR,
    OBSERVATION_TABLE_VAR, ObservationReconcilerConfigError, PLANE_VAR, RECONCILE_PAGE_VAR,
    REGION_VAR, REQUIRED_VARS, USAGE_QUEUE_URL_VAR,
};
use observation_reconciler::duty::{
    BatchOutcome, DueItem, DutyEngine, DutySettings, ItemId, ItemKey,
};
use observation_reconciler::handler::{Invocation, classify, partial_batch_body};
use observation_reconciler::health::{health_body, readiness_body};
use observation_reconciler::{
    ObservationReconcilerRunError, REQUIRED_PROBES, ROLE, compose, role_for,
};

// --- configuration ----------------------------------------------------------

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (PLANE_VAR, "dev".to_owned()),
        (REGION_VAR, "eu-west-1".to_owned()),
        (OBSERVATION_TABLE_VAR, "observation-authority".to_owned()),
        (OBSERVATION_BUCKET_VAR, "aex-dev-observations".to_owned()),
        (DUTY_VAR, "spool.repair".to_owned()),
        (RECONCILE_PAGE_VAR, "100".to_owned()),
        (DUTY_SHARDS_VAR, "16".to_owned()),
        (MAX_ATTEMPTS_VAR, "12".to_owned()),
        (
            USAGE_QUEUE_URL_VAR,
            "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage.fifo".to_owned(),
        ),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ObservationReconcilerConfigError> {
    Config::from_lookup(|name| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_starts_and_carries_no_default() {
    let config = read(&complete()).expect("a complete environment starts");
    assert_eq!(config.plane, "dev");
    assert_eq!(config.region, Region::EuWest1);
    assert_eq!(config.observation_table, "observation-authority");
    assert_eq!(config.observation_bucket, "aex-dev-observations");
    assert_eq!(config.duty, ControlDomain::SpoolRepair);
    assert_eq!(config.page, 100);
    assert_eq!(config.shards, 16);
    assert_eq!(config.max_attempts, 12);
}

#[test]
fn every_missing_variable_is_named() {
    for name in REQUIRED_VARS {
        let mut vars = complete();
        vars.remove(*name);
        assert_eq!(
            read(&vars),
            Err(ObservationReconcilerConfigError::Missing { name }),
            "removing {name}"
        );
    }
}

#[test]
fn every_required_variable_has_a_fixture_and_the_two_lists_agree() {
    let vars = complete();
    for name in REQUIRED_VARS {
        assert!(vars.contains_key(*name), "{name} has no fixture value");
    }
    assert_eq!(vars.len(), REQUIRED_VARS.len());
}

#[test]
fn a_blank_resource_identifier_is_missing_rather_than_empty() {
    for name in [OBSERVATION_TABLE_VAR, OBSERVATION_BUCKET_VAR] {
        let mut vars = complete();
        vars.insert(name, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ObservationReconcilerConfigError::Missing { name })
        );
    }
}

#[test]
fn an_unknown_duty_is_refused_by_name() {
    let mut vars = complete();
    vars.insert(DUTY_VAR, "materialize".to_owned());
    let error = read(&vars).expect_err("an unknown duty is refused");
    assert!(
        matches!(
            error,
            ObservationReconcilerConfigError::Invalid { name: DUTY_VAR, .. }
        ),
        "{error:?}"
    );
}

#[test]
fn the_export_launch_duty_belongs_to_the_launcher_and_is_refused_here() {
    let mut vars = complete();
    vars.insert(DUTY_VAR, ControlDomain::ExportLaunch.as_str().to_owned());
    let error = read(&vars).expect_err("`export.launch` is the launcher's duty");
    match error {
        ObservationReconcilerConfigError::Invalid { name, reason } => {
            assert_eq!(name, DUTY_VAR);
            assert!(
                reason.contains("observation-export-launcher"),
                "the refusal must name the deployable that owns it: {reason}"
            );
        }
        other @ ObservationReconcilerConfigError::Missing { .. } => {
            panic!("expected an invalid-duty refusal, got {other:?}")
        }
    }
}

#[test]
fn every_other_duty_in_the_closed_vocabulary_is_accepted() {
    for duty in ControlDomain::ALL {
        if *duty == ControlDomain::ExportLaunch {
            continue;
        }
        let mut vars = complete();
        vars.insert(DUTY_VAR, duty.as_str().to_owned());
        let config = read(&vars).unwrap_or_else(|error| panic!("{duty:?} refused: {error}"));
        assert_eq!(config.duty, *duty);
    }
}

#[test]
fn the_page_shard_and_attempt_bounds_are_enforced_at_both_ends() {
    for (name, low, high) in [
        (RECONCILE_PAGE_VAR, "0", "1001"),
        (DUTY_SHARDS_VAR, "0", "65"),
        (
            MAX_ATTEMPTS_VAR,
            "0",
            &(limits::OBS_SPOOL_MAX_ATTEMPTS + 1).to_string(),
        ),
    ] {
        for value in [low, high] {
            let mut vars = complete();
            vars.insert(name, value.to_owned());
            let error = read(&vars).unwrap_err();
            assert!(
                matches!(error, ObservationReconcilerConfigError::Invalid { name: reported, .. } if reported == name),
                "{name} = {value} was not refused: {error:?}"
            );
        }
    }
}

#[test]
fn the_attempt_ceiling_may_never_exceed_the_registered_spool_ceiling() {
    let mut vars = complete();
    vars.insert(MAX_ATTEMPTS_VAR, limits::OBS_SPOOL_MAX_ATTEMPTS.to_string());
    let config = read(&vars).expect("the registered ceiling itself is admissible");
    assert_eq!(config.max_attempts, limits::OBS_SPOOL_MAX_ATTEMPTS);
}

#[test]
fn a_plaintext_usage_queue_is_refused() {
    let mut vars = complete();
    vars.insert(
        USAGE_QUEUE_URL_VAR,
        "http://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage.fifo".to_owned(),
    );
    assert!(matches!(
        read(&vars),
        Err(ObservationReconcilerConfigError::Invalid {
            name: USAGE_QUEUE_URL_VAR,
            ..
        })
    ));
}

#[test]
fn an_unknown_plane_and_an_unknown_region_are_both_refused() {
    let mut vars = complete();
    vars.insert(PLANE_VAR, "staging".to_owned());
    assert!(matches!(
        read(&vars),
        Err(ObservationReconcilerConfigError::Invalid {
            name: PLANE_VAR,
            ..
        })
    ));
    let mut vars = complete();
    vars.insert(REGION_VAR, "eu-central-9".to_owned());
    assert!(matches!(
        read(&vars),
        Err(ObservationReconcilerConfigError::Invalid {
            name: REGION_VAR,
            ..
        })
    ));
}

// --- capability grant -------------------------------------------------------

#[test]
fn only_the_deletion_duty_deployment_may_delete_an_object() {
    for duty in ControlDomain::ALL {
        let role = role_for(*duty);
        let deletes = *duty == ControlDomain::DeletionExecute;
        assert_eq!(
            role.holds(Capability::DeleteBodies),
            deletes,
            "`{}` selected `{role:?}`",
            duty.as_str()
        );
        if deletes {
            assert_eq!(role, Role::ReconcilerDeletion);
            continue;
        }
        assert_eq!(role, ROLE);
        let error = compose(role, &[Capability::DeleteBodies], REQUIRED_PROBES)
            .expect_err("a non-deletion duty must refuse the delete grant");
        assert!(
            matches!(error, ObservationReconcilerRunError::Capability { .. }),
            "{error:?}"
        );
    }
}

#[test]
fn no_duty_deployment_may_launch_an_export_task_or_write_an_export_object() {
    for duty in ControlDomain::ALL {
        let role = role_for(*duty);
        for forbidden in [
            Capability::LaunchExportTasks,
            Capability::WriteExportObjects,
        ] {
            assert!(
                !role.holds(forbidden),
                "`{}` must not hold `{}`",
                duty.as_str(),
                forbidden.as_str()
            );
            let error =
                compose(role, &[forbidden], REQUIRED_PROBES).expect_err("outside the grant");
            assert!(
                matches!(error, ObservationReconcilerRunError::Capability { .. }),
                "{error:?}"
            );
        }
    }
}

#[test]
fn each_selected_role_starts_under_its_own_grant_and_the_full_probe_set() {
    for duty in ControlDomain::ALL {
        let role = role_for(*duty);
        compose(role, role.granted(), REQUIRED_PROBES).expect("its own grant starts");
    }
    assert_eq!(
        REQUIRED_PROBES,
        &[
            aex_observation_store_dynamodb::health::Probe::ObservationTable,
            aex_observation_store_dynamodb::health::Probe::ObservationBucket,
        ]
    );
}

#[test]
fn an_unproven_probe_is_never_assumed() {
    let first = REQUIRED_PROBES.first().expect("a probe set is declared");
    let error = compose(ROLE, ROLE.granted(), &[]).expect_err("refused");
    match error {
        ObservationReconcilerRunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
        other => panic!("expected a readiness failure, got {other:?}"),
    }
    let error = compose(ROLE, ROLE.granted(), &REQUIRED_PROBES[..1]).expect_err("refused");
    assert!(
        matches!(error, ObservationReconcilerRunError::NotReady { .. }),
        "{error:?}"
    );
}

// --- the partial-batch response --------------------------------------------

fn outcome() -> BatchOutcome {
    let mut outcome = BatchOutcome::default();
    outcome.succeed(ItemId::new("msg-ok-1"));
    outcome.fail(ItemId::new("msg-bad-1"), "the index count did not settle");
    outcome.succeed(ItemId::new("msg-ok-2"));
    outcome.fail(
        ItemId::new("msg-bad-2"),
        "the outbox fact was not delivered",
    );
    outcome
}

#[test]
fn a_partial_batch_reports_exactly_the_failed_ids_and_never_a_succeeded_one() {
    let body = partial_batch_body(&outcome());
    let failures = body["batchItemFailures"]
        .as_array()
        .expect("a partial-batch body always carries the array");
    let reported: Vec<&str> = failures
        .iter()
        .map(|entry| {
            entry["itemIdentifier"]
                .as_str()
                .expect("every entry names an item")
        })
        .collect();
    assert_eq!(reported, vec!["msg-bad-1", "msg-bad-2"]);
    for succeeded in ["msg-ok-1", "msg-ok-2"] {
        assert!(
            !reported.contains(&succeeded),
            "`{succeeded}` succeeded and must never be re-driven"
        );
    }
    assert_eq!(body.as_object().expect("an object").len(), 1);
}

#[test]
fn an_outcome_with_no_failure_reports_an_empty_array_rather_than_nothing() {
    let mut clean = BatchOutcome::default();
    clean.succeed(ItemId::new("msg-ok-1"));
    let body = partial_batch_body(&clean);
    assert_eq!(
        body["batchItemFailures"]
            .as_array()
            .expect("the array is always present")
            .len(),
        0
    );
}

#[test]
fn merging_two_outcomes_keeps_both_verdicts_apart() {
    let mut first = BatchOutcome::default();
    first.succeed(ItemId::new("a"));
    let mut second = BatchOutcome::default();
    second.fail(ItemId::new("b"), "unresolved");
    first.merge(second);
    assert_eq!(first.succeeded.len(), 1);
    assert_eq!(first.failed.len(), 1);
    assert_eq!(first.failed[0].0.as_str(), "b");
    assert_eq!(first.failed[0].1.as_ref(), "unresolved");
}

// --- the lambda event surface ----------------------------------------------

#[test]
fn an_empty_or_scheduled_event_runs_one_bounded_scan() {
    assert_eq!(classify(&serde_json::json!({})), Invocation::Scheduled);
    assert_eq!(
        classify(&serde_json::json!({
            "source": "aws.events",
            "detail-type": "Scheduled Event",
            "detail": {}
        })),
        Invocation::Scheduled
    );
}

#[test]
fn an_sqs_event_names_every_record_and_never_silently_drops_one() {
    let event = serde_json::json!({
        "Records": [
            {
                "messageId": "msg-1",
                "eventSource": "aws:sqs",
                "body": "{\"pk\":\"SPOOL#wsp_1#03\",\"sk\":\"2026-08-01T09:00:00.000Z#bch_1\"}"
            },
            {
                "messageId": "msg-2",
                "eventSource": "aws:sqs",
                "body": "not json"
            }
        ]
    });
    match classify(&event) {
        Invocation::Records(records) => {
            assert_eq!(
                records.len(),
                2,
                "an unparsable record is kept, not dropped"
            );
            assert_eq!(records[0].message_id.as_str(), "msg-1");
            assert_eq!(
                records[0].target.as_ref().expect("a parsed target").pk,
                "SPOOL#wsp_1#03"
            );
            assert_eq!(records[1].message_id.as_str(), "msg-2");
            assert!(
                records[1].target.is_none(),
                "an unparsable body has no target and is reported as a failure"
            );
        }
        other => panic!("expected queue records, got {other:?}"),
    }
}

#[test]
fn the_two_health_paths_are_read_from_the_shared_constants() {
    assert_eq!(
        classify(&serde_json::json!({ "rawPath": HEALTHZ })),
        Invocation::Health
    );
    assert_eq!(
        classify(&serde_json::json!({ "path": READYZ })),
        Invocation::Readiness
    );
    assert_eq!(HEALTHZ, "/internal/healthz");
    assert_eq!(READYZ, "/internal/readyz");
}

#[test]
fn readiness_names_the_outstanding_probe_and_never_answers_ready_early() {
    let (status, body) = readiness_body("sha256:abc", &[]);
    assert_eq!(status, 503);
    assert_eq!(body["status"].as_str(), Some("not_ready"));
    assert_eq!(
        body["outstanding"].as_str(),
        Some(REQUIRED_PROBES[0].as_str())
    );

    let (status, body) = readiness_body("sha256:abc", REQUIRED_PROBES);
    assert_eq!(status, 200);
    assert_eq!(body["status"].as_str(), Some("ready"));
    assert_eq!(body["releaseDigest"].as_str(), Some("sha256:abc"));

    let live = health_body("sha256:abc");
    assert_eq!(live["status"].as_str(), Some("healthy"));
    assert_eq!(live["releaseDigest"].as_str(), Some("sha256:abc"));
}

// --- the serialized request shapes ------------------------------------------

const TABLE: &str = "dev-eu-west-1-observation-authority";

/// An engine whose whole transport is `transport`; nothing reaches AWS.
fn engine(duty: ControlDomain, transport: impl HttpClient + Clone + 'static) -> DutyEngine {
    use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials};

    let credentials = Credentials::new("AKIDTESTTESTTESTTEST", "test-secret", None, None, "aex");
    let dynamodb = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
            .credentials_provider(credentials.clone())
            .http_client(transport.clone())
            .build(),
    );
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("eu-west-1"))
            .credentials_provider(credentials.clone())
            .http_client(transport.clone())
            .build(),
    );
    let sqs = aws_sdk_sqs::Client::from_conf(
        aws_sdk_sqs::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(aws_sdk_sqs::config::Region::new("eu-west-1"))
            .credentials_provider(credentials)
            .http_client(transport)
            .build(),
    );
    DutyEngine::new(dynamodb, s3, sqs, settings(duty))
}

/// An engine that captures exactly one request.
fn capturing(duty: ControlDomain) -> (DutyEngine, CaptureRequestReceiver) {
    let (transport, receiver) = capture_request(None);
    (engine(duty, transport), receiver)
}

/// An engine that answers `count` calls and records every request it made.
fn replaying(duty: ControlDomain, count: usize) -> (DutyEngine, StaticReplayClient) {
    let events = (0..count)
        .map(|_| {
            ReplayEvent::new(
                http::Request::builder()
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request shape"),
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from("{}"))
                    .expect("an empty successful response"),
            )
        })
        .collect();
    let transport = StaticReplayClient::new(events);
    (engine(duty, transport.clone()), transport)
}

/// The serialized body of one recorded request.
fn body_of(
    request: &aws_smithy_runtime_api::client::orchestrator::HttpRequest,
) -> serde_json::Value {
    serde_json::from_slice(
        request
            .body()
            .bytes()
            .expect("the request body is always in memory"),
    )
    .expect("the request body is JSON")
}

fn settings(duty: ControlDomain) -> DutySettings {
    DutySettings {
        table: TABLE.to_owned(),
        bucket: "aex-dev-observations".to_owned(),
        usage_queue_url: "https://sqs.eu-west-1.amazonaws.com/1/aex-dev-usage.fifo".to_owned(),
        region: Region::EuWest1,
        duty,
        page: 100,
        shards: 16,
        max_attempts: limits::OBS_SPOOL_MAX_ATTEMPTS,
    }
}

fn captured(receiver: CaptureRequestReceiver) -> serde_json::Value {
    let request = receiver.expect_request();
    let bytes = request
        .body()
        .bytes()
        .expect("the request body is always in memory");
    serde_json::from_slice(bytes).expect("the request body is JSON")
}

fn now() -> Timestamp {
    Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
}

#[tokio::test]
async fn a_due_scan_queries_the_sparse_control_index_and_never_scans() {
    let (engine, receiver) = capturing(ControlDomain::SpoolRepair);
    let _ignored = engine.due_page(3, now()).await;

    let body = captured(receiver);
    assert_eq!(body["TableName"].as_str(), Some(TABLE));
    assert_eq!(body["IndexName"].as_str(), Some(Index::Control.as_str()));
    let condition = body["KeyConditionExpression"]
        .as_str()
        .expect("a key condition");
    assert!(
        !condition.contains("CTRL#"),
        "the partition never reaches the expression string: {condition}"
    );
    let values = &body["ExpressionAttributeValues"];
    let partition = values
        .as_object()
        .expect("bound values")
        .values()
        .filter_map(|value| value["S"].as_str())
        .find(|text| text.starts_with("CTRL#"))
        .expect("the control partition is bound as a value");
    assert_eq!(
        partition,
        keys::control_pk(ControlDomain::SpoolRepair, 3),
        "the due partition is always sharded"
    );
    assert_eq!(body["Limit"].as_i64(), Some(100));
}

#[tokio::test]
async fn a_claim_is_conditional_on_the_observed_attempt_count() {
    let (engine, receiver) = capturing(ControlDomain::SpoolRepair);
    let item = DueItem {
        id: ItemId::new("bch_1"),
        key: ItemKey {
            pk: "SPOOL#wsp_1#03".to_owned(),
            sk: "2026-08-01T09:00:00.000Z#bch_1".to_owned(),
        },
        attempts: 3,
        attributes: HashMap::new(),
    };
    let _ignored = engine.claim(&item, now()).await;

    let body = captured(receiver);
    assert_eq!(body["TableName"].as_str(), Some(TABLE));
    let condition = body["ConditionExpression"]
        .as_str()
        .expect("a claim is always conditional");
    assert!(
        condition.contains("attribute_not_exists("),
        "a first claim must still be admissible: {condition}"
    );
    let update = body["UpdateExpression"].as_str().expect("an update");
    let names: Vec<&str> = body["ExpressionAttributeNames"]
        .as_object()
        .expect("bound names")
        .values()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(names.contains(&"attempts"), "{names:?}");
    assert!(names.contains(&"nextAttemptAt"), "{names:?}");
    assert!(
        names.contains(&Index::Control.sort_key()),
        "the claim pushes the item out of the due window: {names:?}"
    );
    assert!(update.starts_with("SET "), "{update}");

    let numbers: Vec<&str> = body["ExpressionAttributeValues"]
        .as_object()
        .expect("bound values")
        .values()
        .filter_map(|value| value["N"].as_str())
        .collect();
    assert!(numbers.contains(&"3"), "the observed count: {numbers:?}");
    assert!(numbers.contains(&"4"), "the bumped count: {numbers:?}");
}

#[tokio::test]
async fn the_claim_backoff_is_the_declared_spool_backoff() {
    let (engine, receiver) = capturing(ControlDomain::SpoolRepair);
    let item = DueItem {
        id: ItemId::new("bch_1"),
        key: ItemKey {
            pk: "SPOOL#wsp_1#03".to_owned(),
            sk: "2026-08-01T09:00:00.000Z#bch_1".to_owned(),
        },
        attempts: 3,
        attributes: HashMap::new(),
    };
    let _ignored = engine.claim(&item, now()).await;

    // Three attempts is an 8 s delay under the pinned exponential backoff.
    let expected = Timestamp::from_unix_millis(now().unix_millis() + 8_000)
        .expect("in range")
        .to_wire();
    let body = captured(receiver);
    let strings: Vec<String> = body["ExpressionAttributeValues"]
        .as_object()
        .expect("bound values")
        .values()
        .filter_map(|value| value["S"].as_str().map(str::to_owned))
        .collect();
    assert!(
        strings.iter().any(|text| text == &expected),
        "the next attempt is pushed to {expected}: {strings:?}"
    );
}

/// A spool chunk that has exhausted every attempt.
fn exhausted() -> DueItem {
    let mut attributes = HashMap::new();
    attributes.insert(
        "scopeKey".to_owned(),
        AttributeValue::S("W#wsp_0000000001e40r2081040g2081".to_owned()),
    );
    attributes.insert(
        "workspaceId".to_owned(),
        AttributeValue::S("wsp_0000000001e40r2081040g2081".to_owned()),
    );
    attributes.insert(
        "lossCandidates".to_owned(),
        AttributeValue::L(vec![AttributeValue::M(HashMap::from([
            (
                "gapId".to_owned(),
                AttributeValue::S("gap_0000000001e40r2081040g2081".to_owned()),
            ),
            ("signal".to_owned(), AttributeValue::S("logs".to_owned())),
            (
                "acceptedSeqLo".to_owned(),
                AttributeValue::N("4".to_owned()),
            ),
            (
                "acceptedSeqHiExclusive".to_owned(),
                AttributeValue::N("9".to_owned()),
            ),
            (
                "attemptedRecords".to_owned(),
                AttributeValue::N("5".to_owned()),
            ),
            (
                "attemptedBytes".to_owned(),
                AttributeValue::N("512".to_owned()),
            ),
            (
                "timeRange".to_owned(),
                AttributeValue::M(HashMap::from([
                    (
                        "gte".to_owned(),
                        AttributeValue::S("2026-08-01T08:59:59.000Z".to_owned()),
                    ),
                    (
                        "lt".to_owned(),
                        AttributeValue::S("2026-08-01T09:00:00.001Z".to_owned()),
                    ),
                ])),
            ),
        ]))]),
    );
    DueItem {
        id: ItemId::new("bch_1"),
        key: ItemKey {
            pk: "SPOOL#wsp_1#03".to_owned(),
            sk: "2026-08-01T09:00:00.000Z#bch_1".to_owned(),
        },
        attempts: limits::OBS_SPOOL_MAX_ATTEMPTS,
        attributes,
    }
}

#[tokio::test]
async fn an_item_past_the_attempt_ceiling_is_quarantined_with_a_durable_reason() {
    let (engine, receiver) = capturing(ControlDomain::BatchExpire);
    let _ignored = engine
        .quarantine(&exhausted(), "the index never settled", now())
        .await;

    let body = captured(receiver);
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(
        update.contains("REMOVE "),
        "a quarantined item leaves the due index: {update}"
    );
    let strings: Vec<&str> = body["ExpressionAttributeValues"]
        .as_object()
        .expect("bound values")
        .values()
        .filter_map(|value| value["S"].as_str())
        .collect();
    assert!(
        strings.contains(&"the index never settled"),
        "the quarantine reason is durable: {strings:?}"
    );
    assert!(strings.contains(&"quarantined"), "{strings:?}");
}

#[tokio::test]
async fn a_quarantined_spool_chunk_escalates_to_a_durable_gap_rather_than_vanishing() {
    let (engine, transport) = replaying(ControlDomain::SpoolRepair, 1);
    engine
        .quarantine(&exhausted(), "the index never settled", now())
        .await
        .expect("the atomic durable write lands");

    let requests: Vec<_> = transport.actual_requests().collect();
    assert_eq!(
        requests.len(),
        1,
        "the gap and source terminalization are one transaction"
    );
    let transaction = body_of(requests[0]);
    let actions = transaction["TransactItems"]
        .as_array()
        .expect("transaction actions");
    assert_eq!(
        actions.len(),
        3,
        "one gap Put, one gap-change hint Update and one source Update"
    );
    let gap = &actions[0]["Put"];
    let item = &gap["Item"];
    assert_eq!(item["itemType"]["S"].as_str(), Some("telemetry_gap"));
    assert_eq!(
        item["reason"]["S"].as_str(),
        Some("spool_lost"),
        "`pipeline_loss` is spelled `spool_lost` on the wire"
    );
    assert_eq!(item["ordinalRange"]["M"]["lo"]["N"].as_str(), Some("4"));
    assert_eq!(item["ordinalRange"]["M"]["hi"]["N"].as_str(), Some("8"));
    assert_eq!(
        item["workspaceId"]["S"].as_str(),
        Some("wsp_0000000001e40r2081040g2081")
    );
    assert_eq!(
        item["gwPk"]["S"].as_str(),
        Some("GAPW#wsp_0000000001e40r2081040g2081")
    );
    assert_eq!(item["attemptedRecords"]["N"].as_str(), Some("5"));
    assert!(
        item.get("timeRange").is_some(),
        "the exact time window is retained"
    );
    assert!(
        item["pk"]["S"]
            .as_str()
            .expect("a gap partition")
            .starts_with("GAP#"),
        "{item}"
    );
    assert_eq!(
        gap["ConditionExpression"].as_str().map(str::to_owned),
        Some("attribute_not_exists(#n0) AND attribute_not_exists(#n1)".to_owned()),
        "a gap revision is immutable"
    );
    let hint = &actions[1]["Update"];
    assert_eq!(
        hint["Key"]["pk"]["S"].as_str(),
        Some("GAPV#wsp_0000000001e40r2081040g2081"),
        "the hint the follow sockets read is advanced by the same transaction"
    );
    assert_eq!(hint["Key"]["sk"]["S"].as_str(), Some("CHANGE"));
    let published = hint["UpdateExpression"]
        .as_str()
        .expect("the hint carries an update");
    assert!(
        published.contains("ADD "),
        "a hint that is set rather than added would lose a concurrent append: {published}"
    );
    assert!(
        hint.get("ConditionExpression").is_none(),
        "the hint may never be the reason a proven gap fails to land: {hint}"
    );

    assert!(
        actions[2]["Update"]["UpdateExpression"]
            .as_str()
            .is_some_and(|update| update.contains("REMOVE ")),
        "the same transaction removes the source from the due index"
    );
    let condition = actions[2]["Update"]["ConditionExpression"]
        .as_str()
        .expect("the source update is fenced");
    assert!(condition.contains("attribute_exists"), "{condition}");
    assert!(condition.contains(" = "), "{condition}");
}

// --- dependency hygiene ------------------------------------------------------

#[test]
fn no_clickhouse_or_kinesis_dependency_can_reach_this_binary() {
    let manifest = include_str!("../Cargo.toml");
    for banned in ["clickhouse", "kinesis"] {
        assert!(
            !manifest.to_ascii_lowercase().contains(banned),
            "`{banned}` must not appear anywhere in this deployable's dependency set"
        );
    }
}

#[test]
fn the_health_paths_are_never_hand_typed_in_this_deployable() {
    for source in [
        include_str!("../src/handler.rs"),
        include_str!("../src/health.rs"),
    ] {
        assert!(
            !source.contains("\"/internal/"),
            "the health paths come from `aex_observation_store_dynamodb::health`, never a literal"
        );
    }
}
