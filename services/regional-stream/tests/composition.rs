//! Start-up, configuration and no-mutation facts for `regional-stream`.

use std::collections::BTreeMap;

use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::router::RouteOwner;
use aex_wire::routes::{Plane, TransportKind, route};
use regional_stream::config::{self, Config, WakeMode};

fn polling() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (config::STREAM_PORT, "8080".to_owned()),
        (
            config::AUTHZ_FUNCTION_ARN,
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-central-authz".to_owned(),
        ),
        (
            config::AUTHZ_VERIFY_KEYS_PARAM,
            "/aex/dev/authz/verify-keys".to_owned(),
        ),
        (
            config::AUTHZ_PROJECTION_TABLE,
            "aex-dev-regional-authz-projection".to_owned(),
        ),
        (
            config::SESSION_TABLE,
            "aex-dev-session-authority".to_owned(),
        ),
        (
            config::OBSERVATION_TABLE,
            "aex-dev-observation-authority".to_owned(),
        ),
        (
            config::CONTENT_BUCKET,
            "aex-dev-eu-west-1-content".to_owned(),
        ),
        (
            config::CURSOR_SIGNING_KEY_REF,
            "/aex/dev/regional/cursor-signing-key".to_owned(),
        ),
        (config::OBS_INDEX_SETTLE_MS, "2000".to_owned()),
        (config::OBS_QUERY_SCANNED_ITEMS, "50000".to_owned()),
        (config::OBS_QUERY_SEGMENTS, "64".to_owned()),
        (config::OBS_QUERY_READ_BYTES, "33554432".to_owned()),
        (config::STREAM_WAKE_MODE, "poll".to_owned()),
        (config::STREAM_MAX_TASKS, "6".to_owned()),
        (config::STREAM_MAX_CONNECTIONS, "1500".to_owned()),
        (config::STREAM_MAX_CONNECTIONS_SESSION, "750".to_owned()),
        (config::STREAM_MAX_CONNECTIONS_OBSERVATION, "750".to_owned()),
        (
            config::STREAM_MAX_CONNECTIONS_PER_WORKSPACE,
            "64".to_owned(),
        ),
        (config::STREAM_CONNECTION_BUFFER_BYTES, "4194304".to_owned()),
        (config::STREAM_WRITE_STALL_MS, "10000".to_owned()),
        (config::STREAM_DRAIN_DEADLINE_MS, "25000".to_owned()),
        (config::ASSERTION_CACHE_BYTES, "1048576".to_owned()),
    ])
}

fn streaming() -> BTreeMap<&'static str, String> {
    let mut vars = polling();
    vars.insert(config::STREAM_WAKE_MODE, "ddb_streams".to_owned());
    vars.insert(config::STREAM_MAX_TASKS, "2".to_owned());
    vars.insert(
        config::SESSION_TABLE_STREAM_ARN,
        "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-session-authority/stream/2026-08-01T00:00:00.000"
            .to_owned(),
    );
    vars.insert(
        config::OBSERVATION_TABLE_STREAM_ARN,
        "arn:aws:dynamodb:eu-west-1:000000000000:table/aex-dev-observation-authority/stream/2026-08-01T00:00:00.000"
            .to_owned(),
    );
    vars.insert(
        config::DYNAMODB_STREAMS_ENDPOINT_URL,
        "https://vpce-0123456789abcdef0.streams.dynamodb.eu-west-1.vpce.amazonaws.com".to_owned(),
    );
    vars
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, RegionalHttpConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn both_wake_modes_accept_their_own_complete_environment() {
    let polled = read(&polling()).expect("poll mode is accepted");
    assert_eq!(polled.wake_mode, WakeMode::Poll);
    assert!(polled.session_stream.is_none());

    let streamed = read(&streaming()).expect("ddb_streams mode is accepted");
    assert_eq!(streamed.wake_mode, WakeMode::DdbStreams);
    assert!(streamed.session_stream.is_some());
    assert!(streamed.dynamodb_streams_endpoint_url.is_some());
    assert_eq!(streamed.port, 8080);
}

#[test]
fn every_required_variable_is_required() {
    let vars = polling();
    assert_eq!(vars.len(), config::REQUIRED.len());
    for name in config::REQUIRED {
        let mut missing = vars.clone();
        missing.remove(name);
        let error = read(&missing).expect_err("a missing variable refuses the process");
        assert!(
            matches!(error, RegionalHttpConfigError::Missing { name: reported } if reported == name),
            "removing {name} reported {error:?}"
        );
    }
}

#[test]
fn ddb_streams_refuses_a_third_reader_task() {
    let mut vars = streaming();
    vars.insert(config::STREAM_MAX_TASKS, "3".to_owned());
    let error = read(&vars).expect_err("a third shard reader is refused");
    let RegionalHttpConfigError::Invalid { name, reason } = error else {
        panic!("expected an invalid-value refusal");
    };
    assert_eq!(name, config::STREAM_MAX_TASKS);
    assert!(reason.contains("ddb_streams"), "{reason}");
    // Polling has no shard-reader bound at all, so the same task count is fine.
    let mut polled = polling();
    polled.insert(config::STREAM_MAX_TASKS, "3".to_owned());
    assert!(read(&polled).is_ok());
}

#[test]
fn ddb_streams_requires_both_authority_streams() {
    let mut vars = streaming();
    vars.remove(config::SESSION_TABLE_STREAM_ARN);
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Missing { name }) if name == config::SESSION_TABLE_STREAM_ARN
    ));
}

#[test]
fn ddb_streams_requires_an_endpoint_specific_private_origin() {
    let mut missing = streaming();
    missing.remove(config::DYNAMODB_STREAMS_ENDPOINT_URL);
    assert!(matches!(
        read(&missing),
        Err(RegionalHttpConfigError::Missing { name }) if name == config::DYNAMODB_STREAMS_ENDPOINT_URL
    ));

    let mut public = streaming();
    public.insert(
        config::DYNAMODB_STREAMS_ENDPOINT_URL,
        "https://streams.dynamodb.eu-west-1.amazonaws.com".to_owned(),
    );
    assert!(matches!(
        read(&public),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::DYNAMODB_STREAMS_ENDPOINT_URL
    ));
}

#[test]
fn a_total_below_a_class_budget_refuses_the_process() {
    let mut vars = polling();
    vars.insert(config::STREAM_MAX_CONNECTIONS, "500".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::STREAM_MAX_CONNECTIONS
    ));
}

#[test]
fn the_stream_cannot_be_bound_to_a_queue_a_work_table_or_a_secret_key() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = polling();
        vars.insert(name, "bound".to_owned());
        let error = read(&vars).expect_err("a mutating binding refuses the stream");
        assert!(
            matches!(
                error,
                RegionalHttpConfigError::Forbidden { name: reported, deployable, .. }
                    if reported == name && deployable == config::DEPLOYABLE
            ),
            "binding {name} reported {error:?}"
        );
    }
}

#[test]
fn the_stream_owns_every_ndjson_route_and_only_those() {
    let owned = RouteOwner::Stream.routes();
    assert_eq!(
        owned.len(),
        24,
        "the stream owns 24 generated NDJSON routes"
    );
    for id in owned {
        let descriptor = route(id);
        assert_eq!(descriptor.plane, Plane::Regional, "`{id}`");
        assert_eq!(descriptor.transport, TransportKind::Ndjson, "`{id}`");
        // A read-only service must not own a route that declares a replay
        // identity: an idempotency key only ever protects a mutation.
        assert_eq!(
            descriptor.idempotency,
            aex_wire::idempotency::IdempotencyKind::None,
            "`{id}` declares a replay identity"
        );
    }
}
