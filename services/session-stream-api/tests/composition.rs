//! Start-up, configuration, capability and route-ownership facts for the merged
//! `session-stream-api`.
//!
//! This file is the union of what `regional-session-api` and `regional-stream`
//! each asserted about their own composition, plus the checks the merge made
//! necessary: the capability manifest that replaced the stream's `AEX_WORK_TABLE`
//! refusal, the process-wide assertion cache split, and the two drain bounds
//! that used to be a comment and a fixture constant.

use std::collections::BTreeMap;

use aex_regional_http::capability::{
    Capability as _, CompositionError, ContentEncrypt, StreamSocket, WorkClaim,
};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::drain::MAX_DRAIN_DEADLINE_MS;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use session_stream_api::capability;
use session_stream_api::config::{self, Config, EDGE_COUNT, WakeMode};

/// The complete `poll`-mode environment, which is what dev runs.
fn polling() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (config::PORT, "8080".to_owned()),
        // Deliberately below the 30 s ECS stop timeout. This is no longer a
        // comment anybody could ignore: `MAX_DRAIN_DEADLINE_MS` is the reader's
        // upper bound, so a deadline above the stop timeout refuses the process.
        (config::DRAIN_DEADLINE_MS, "25000".to_owned()),
        (
            config::CREDENTIAL_PEPPER_REF,
            "aex/dev/central/token-pepper".to_owned(),
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
            config::CONTENT_BUCKET,
            "aex-dev-eu-west-1-content".to_owned(),
        ),
        (
            config::CURSOR_SIGNING_KEY_REF,
            "/aex/dev/regional/cursor-signing-key".to_owned(),
        ),
        (
            config::REGIONAL_API_URL,
            "https://eu-west-1.api.aex.test".to_owned(),
        ),
        (config::WORK_TABLE, "aex-dev-regional-work".to_owned()),
        (config::CONTENT_TABLE, "aex-dev-regional-content".to_owned()),
        (
            config::REGISTRY_TABLE,
            "aex-dev-regional-registry".to_owned(),
        ),
        (
            config::SECRET_CUSTODY_TABLE,
            "aex-dev-regional-secret-custody".to_owned(),
        ),
        (
            config::RUNTIME_ACTIVITY_TABLE,
            "aex-dev-runtime-activity".to_owned(),
        ),
        (
            config::USAGE_COMPUTE_QUEUE_URL,
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-compute".to_owned(),
        ),
        (
            config::USAGE_STORAGE_QUEUE_URL,
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-storage".to_owned(),
        ),
        (config::RUNTIME_DUE_SHARDS, "8".to_owned()),
        (config::RUNTIME_DUE_PAGE_ITEMS, "32".to_owned()),
        (config::RUNTIME_DUE_PAGE_READS, "64".to_owned()),
        (config::PRICING_VERSION, "synthetic-zero-v1".to_owned()),
        (
            config::USAGE_QUERY_TABLE,
            "aex-dev-usage-query-projection".to_owned(),
        ),
        (config::CONTENT_BUCKET_OWNER, "000000000000".to_owned()),
        (
            config::CONTENT_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"
                .to_owned(),
        ),
        (config::MAX_JSON_BODY_BYTES, "65536".to_owned()),
        (config::MAX_PAGE_ITEMS, "100".to_owned()),
        (config::MAX_PAGE_BYTES, "1048576".to_owned()),
        (
            config::OBSERVATION_TABLE,
            "aex-dev-observation-authority".to_owned(),
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
        // 9 s, not the 10 s the stream fixture used to carry: with a 15 s poll
        // ceiling and a 25 s drain deadline, 10 s left the last producer exactly
        // zero slack. See `a_write_stall_that_cannot_be_observed_inside_the_drain_is_refused`.
        (config::STREAM_WRITE_STALL_MS, "9000".to_owned()),
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

// --- configuration: both halves' variables in one process ---------------------

#[test]
fn a_complete_environment_is_accepted() {
    let config = read(&polling()).expect("the complete environment is accepted");
    assert_eq!(config.region.as_str(), "eu-west-1");
    assert_eq!(
        config.regional_api_url.as_str(),
        "https://eu-west-1.api.aex.test"
    );
    assert_eq!(config.limits().json_body_bytes, 65_536);
    assert_eq!(config.limits().query_page_items, 100);
    assert_eq!(config.observation_table, "aex-dev-observation-authority");
    assert_eq!(config.work_table, "aex-dev-regional-work");
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
    assert_eq!(streamed.port, 8_080);
}

#[test]
fn every_required_variable_is_required() {
    let vars = polling();
    assert_eq!(
        vars.len(),
        config::REQUIRED.len(),
        "the fixture covers exactly the declared variable set"
    );
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

/// The two halves used to name the same container port and the same drain
/// window under four different variables. Two of those names are now gone.
#[test]
fn the_collapsed_variable_names_are_the_only_ones_read() {
    for retired in [
        "AEX_SESSION_PORT",
        "AEX_STREAM_PORT",
        "AEX_SESSION_DRAIN_DEADLINE_MS",
        "AEX_STREAM_DRAIN_DEADLINE_MS",
    ] {
        assert!(
            !config::REQUIRED.contains(&retired),
            "`{retired}` survived the merge"
        );
    }
    assert!(config::REQUIRED.contains(&config::PORT));
    assert!(config::REQUIRED.contains(&config::DRAIN_DEADLINE_MS));

    // A retired name is inert rather than forbidden: binding one is a stale task
    // definition, not a capability breach, and the missing-variable refusal for
    // the new name already names what to fix.
    let mut vars = polling();
    vars.insert("AEX_STREAM_PORT", "9999".to_owned());
    assert_eq!(
        read(&vars).expect("a stale name is ignored").port,
        8_080,
        "the retired name must not be read"
    );
}

#[test]
fn a_port_outside_the_tcp_range_refuses_the_process() {
    for value in ["0", "65536", "not-a-port"] {
        let mut vars = polling();
        vars.insert(config::PORT, value.to_owned());
        assert!(
            matches!(
                read(&vars),
                Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::PORT
            ),
            "port `{value}` was admitted"
        );
    }
}

#[test]
fn a_non_https_regional_api_url_is_refused() {
    let mut vars = polling();
    vars.insert(config::REGIONAL_API_URL, "http://localhost".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::REGIONAL_API_URL
    ));
}

#[test]
fn a_bucket_owned_by_another_account_refuses_the_process() {
    let mut vars = polling();
    vars.insert(config::CONTENT_BUCKET_OWNER, "999999999999".to_owned());
    let error = read(&vars).expect_err("a cross-account bucket owner is refused");
    assert!(
        matches!(error, RegionalHttpConfigError::Invalid { name, .. } if name == config::CONTENT_BUCKET_OWNER),
        "{error:?}"
    );
}

#[test]
fn a_page_bound_outside_the_registry_range_refuses_the_process() {
    let mut vars = polling();
    vars.insert(config::MAX_PAGE_ITEMS, "1001".to_owned());
    assert!(matches!(
        read(&vars),
        Err(RegionalHttpConfigError::Invalid { name, .. }) if name == config::MAX_PAGE_ITEMS
    ));
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

// --- the drain bounds, which used to be a comment and a fixture constant ------

/// The relationship the ECS stop timeout has with the process's own deadline is
/// now enforced by the configuration reader, not asserted once in a fixture.
#[test]
fn a_drain_deadline_that_could_never_fire_refuses_the_process() {
    // `aex_regional_http::drain` asserts the arithmetic at compile time; what is
    // proved here is that the configuration reader actually enforces it.
    //
    // The old bound admitted 600_000 ms — twenty times the stop timeout — so a
    // deployment could ask for a ten-minute drain and simply be killed at 30 s.
    for value in ["999", "30000", "600000"] {
        let mut vars = polling();
        vars.insert(config::DRAIN_DEADLINE_MS, value.to_owned());
        assert!(
            matches!(
                read(&vars),
                Err(RegionalHttpConfigError::Invalid { name, .. })
                    if name == config::DRAIN_DEADLINE_MS
            ),
            "drain deadline `{value}` was admitted"
        );
    }

    let config = read(&polling()).expect("the complete environment is accepted");
    assert_eq!(config.drain_deadline_ms, 25_000);
    assert!(config.drain_deadline_ms <= MAX_DRAIN_DEADLINE_MS);
}

/// A producer observes the drain flag only at the top of its loop, whose wait
/// runs to `LISTEN_POLL_MAX`, and may then block for the whole write stall on a
/// wedged reader. If that sum reaches the deadline, the last socket provably
/// cannot be drained — and in the merged process that overrun ends the session
/// half too.
#[test]
fn a_write_stall_that_cannot_be_observed_inside_the_drain_is_refused() {
    let poll_max_ms =
        u64::try_from(regional_observation_api::api::LISTEN_POLL_MAX.as_millis()).expect("ms");

    // The value the stream's own fixture used to carry: 15 s + 10 s = exactly
    // the 25 s deadline, leaving zero slack.
    let mut exact = polling();
    exact.insert(config::STREAM_WRITE_STALL_MS, "10000".to_owned());
    let error = read(&exact).expect_err("a stall with zero slack is refused");
    assert!(
        matches!(
            &error,
            RegionalHttpConfigError::Invalid { name, .. }
                if *name == config::STREAM_WRITE_STALL_MS
        ),
        "{error:?}"
    );

    let mut over = polling();
    over.insert(config::STREAM_WRITE_STALL_MS, "20000".to_owned());
    assert!(read(&over).is_err(), "a stall past the deadline is refused");

    let config = read(&polling()).expect("the complete environment is accepted");
    assert!(
        poll_max_ms + config.write_stall_ms < config.drain_deadline_ms,
        "the admitted fixture must leave the last producer real slack"
    );
}

// --- two edges, one credential check ------------------------------------------

/// The two audiences this process serves stay distinct.
///
/// They used to be kept apart by the assertion each edge asked `central-authz`
/// for, and by a per-edge cache funded from a process-wide byte budget. Both are
/// gone: there is nothing cached between requests, so the budget was deleted
/// rather than divided, and what separates the edges is the audience each
/// requires the projected key row to name.
#[test]
fn the_two_edges_are_two_audiences_and_no_process_wide_budget() {
    let config = read(&polling()).expect("the complete environment is accepted");
    assert_eq!(EDGE_COUNT, 2);
    assert_eq!(config.credential_pepper_ref, "aex/dev/central/token-pepper");
    // One ring, read once, shared by both edges: a second read could give one
    // half a pepper set the other does not hold mid-rotation.
    let mut absent = polling();
    absent.remove(config::CREDENTIAL_PEPPER_REF);
    assert!(matches!(
        read(&absent),
        Err(RegionalHttpConfigError::Missing { name }) if name == config::CREDENTIAL_PEPPER_REF
    ));
}

// --- capability: what replaced the deleted `AEX_WORK_TABLE` refusal -----------

/// The three refusals that survived. `AEX_WORK_TABLE` is deliberately not among
/// them: the session half requires it, so the environment can no longer carry
/// the stream's no-write guarantee.
#[test]
fn the_edge_cannot_be_bound_to_a_queue_or_the_secret_key() {
    assert_eq!(config::FORBIDDEN.len(), 3);
    for (name, _) in config::FORBIDDEN {
        let mut vars = polling();
        vars.insert(name, "bound".to_owned());
        let error = read(&vars).expect_err("a forbidden binding refuses the process");
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
fn the_work_table_is_required_rather_than_forbidden_and_the_capability_names_it() {
    assert!(
        config::REQUIRED.contains(&config::WORK_TABLE),
        "the session half requires the work table"
    );
    assert!(
        !config::FORBIDDEN
            .iter()
            .any(|(name, _)| *name == config::WORK_TABLE),
        "one process cannot both require and forbid a variable"
    );

    // What replaced it: the work table is bound to a named capability, and the
    // manifest is what a reader consults to answer "where is the write".
    let manifest = capability::manifest().expect("the deployable id is well formed");
    assert!(manifest.capabilities.contains(WorkClaim::ID));
    assert!(manifest.capabilities.contains(StreamSocket::ID));
    assert!(manifest.capabilities.contains(ContentEncrypt::ID));
}

#[test]
fn the_declared_manifest_admits_the_complete_environment() {
    let config = read(&polling()).expect("the complete environment is accepted");
    capability::admit(&config).expect("the declared capabilities match the resolved resources");
}

/// `admit` is fail-closed in both directions: a resolved key with no manifest
/// binding is an undeclared key, and a manifest binding with no resolved value
/// is a missing one. That is what stops the two lists drifting apart.
#[test]
fn the_capability_check_is_fail_closed_in_both_directions() {
    let config = read(&polling()).expect("the complete environment is accepted");
    let manifest = capability::manifest().expect("the deployable id is well formed");

    let mut extra = capability::resolved(&config);
    extra
        .values
        .insert("AEX_SOMETHING_NEW".to_owned(), "bound".to_owned());
    assert!(matches!(
        aex_regional_http::capability::admit(&manifest, &extra),
        Err(CompositionError::UndeclaredKey(_))
    ));

    let mut absent = capability::resolved(&config);
    absent.values.remove(config::WORK_TABLE);
    assert!(matches!(
        aex_regional_http::capability::admit(&manifest, &absent),
        Err(CompositionError::MissingBinding(name)) if name == config::WORK_TABLE
    ));

    // An off-plane content key is the residency failure the ARN binding exists
    // for: it type-checks, deploys and then encrypts in the wrong region.
    let mut off_plane = capability::resolved(&config);
    off_plane.values.insert(
        config::CONTENT_KMS_KEY_ARN.to_owned(),
        "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555".to_owned(),
    );
    assert!(matches!(
        aex_regional_http::capability::admit(&manifest, &off_plane),
        Err(CompositionError::OffPlaneArn(name)) if name == config::CONTENT_KMS_KEY_ARN
    ));
}

// --- route ownership: unchanged by the merge, on purpose ----------------------

#[test]
fn the_finite_half_owns_every_regional_non_stream_route_except_the_peers() {
    let owned = RouteOwner::SessionApi.routes();
    assert!(
        !owned.is_empty(),
        "the finite half owns no generated routes"
    );
    for id in &owned {
        let descriptor = route(*id);
        assert_eq!(descriptor.plane, Plane::Regional, "`{id}`");
        assert!(
            matches!(
                descriptor.transport,
                TransportKind::Unary | TransportKind::Binary
            ),
            "`{id}` is a frame stream"
        );
    }
    // Provider credential registration still belongs to the plaintext-bearing
    // secret edge, and this deployable must not claim it.
    assert_eq!(
        route_owner(RouteId::ProviderCredentialRegister),
        Some(RouteOwner::SecretApi)
    );
}

#[test]
fn the_owned_set_is_drawn_from_the_groups_and_not_from_a_second_list() {
    let from_groups: Vec<RouteId> = RouteGroup::ALL
        .iter()
        .flat_map(|group| RouteOwner::SessionApi.routes_in(*group))
        .collect();
    let mut expected = from_groups;
    expected.sort_unstable_by_key(|id| *id as usize);
    assert_eq!(
        expected,
        RouteOwner::SessionApi.routes(),
        "iterating the groups reproduces the owned set exactly"
    );
}

#[test]
fn the_stream_half_owns_every_ndjson_route_and_only_those() {
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
        // A read-only half must not own a route that declares a replay identity:
        // an idempotency key only ever protects a mutation.
        assert_eq!(
            descriptor.idempotency,
            aex_wire::idempotency::IdempotencyKind::None,
            "`{id}` declares a replay identity"
        );
    }
}

/// The merge joined two route sets in one process and in one artifact name, but
/// not in one owner. Both [`RouteOwner`] variants survive and their templates
/// stay disjoint, which is what keeps a later split a unit-registry change
/// rather than a rewrite of two mount strategies.
#[test]
fn the_two_owners_remain_distinct_and_their_templates_stay_disjoint() {
    let unary = RouteOwner::SessionApi.routes();
    let ndjson = RouteOwner::Stream.routes();
    assert!(
        unary.iter().all(|id| !ndjson.contains(id)),
        "one route cannot belong to both halves"
    );
    assert_eq!(
        RouteOwner::SessionApi.deployable(),
        RouteOwner::Stream.deployable(),
        "both halves deploy as the merged artifact"
    );
    assert_ne!(
        RouteOwner::SessionApi.half(),
        RouteOwner::Stream.half(),
        "a mount refusal must still name which half it meant"
    );

    // `Router::merge` panics on an overlapping method route, so the two mounts
    // are only safe while no template is claimed by both halves.
    let unary_templates: Vec<(&str, &str)> = unary
        .iter()
        .map(|id| (route(*id).template, route(*id).operation_id))
        .collect();
    for id in &ndjson {
        let stream_template = route(*id).template;
        assert!(
            !unary_templates
                .iter()
                .any(|(template, _)| *template == stream_template),
            "`{stream_template}` is claimed by both halves and would panic `Router::merge`"
        );
    }
}
