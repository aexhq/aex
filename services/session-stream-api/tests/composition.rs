//! Start-up, capability, and route-ownership facts for `session-stream-api`.

use std::collections::BTreeMap;

use aex_regional_http::capability::{
    Capability as _, CompositionError, ContentEncrypt, SecretPlaintextAdmission,
    SessionOperationInvoke, WorkClaim,
};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::routes::{Plane, RouteId, TransportKind, route};
use aex_wire::server::RouteGroup;
use session_stream_api::capability;
use session_stream_api::config::{self, Config, EDGE_COUNT};

fn catalog_json() -> String {
    let variants = [
        ("512mb", 512),
        ("1gb", 1_024),
        ("2gb", 2_048),
        ("4gb", 4_096),
        ("8gb", 8_192),
    ];
    let rows = variants
        .into_iter()
        .enumerate()
        .map(|(index, (variant, memory))| {
            (
                variant,
                serde_json::json!({
                    "imageArn": format!(
                        "arn:aws:lambda:eu-west-1:000000000000:microvm-image:aex-dev-{}",
                        char::from(b'a' + u8::try_from(index).expect("five rows")).to_string().repeat(52),
                    ),
                    "imageVersion": (index + 1).to_string(),
                    "artifactDigest": format!("sha256:{index:064x}"),
                    "minimumMemoryMiB": memory,
                    "browser": false,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    serde_json::to_string(&rows).expect("catalog JSON")
}

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (config::PLANE, "dev".to_owned()),
        (config::REGION, "eu-west-1".to_owned()),
        (config::RELEASE_DIGEST, "sha256:deadbeef".to_owned()),
        (config::PORT, "8080".to_owned()),
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
            config::SESSION_TELEMETRY_BUCKET,
            "aex-dev-eu-west-1-session-telemetry".to_owned(),
        ),
        (
            config::SESSION_TELEMETRY_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/99999999-8888-7777-6666-555555555555"
                .to_owned(),
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
        (
            config::SESSION_OPERATION_WORKER_FUNCTION_ARN,
            "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-session-operation-worker:live"
                .to_owned(),
        ),
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
            config::SECRET_KEYSTORE_TABLE,
            "aex-dev-regional-secret-keystore".to_owned(),
        ),
        (
            config::SECRET_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                .to_owned(),
        ),
        (config::BRANCH_KEY_CACHE_BYTES, "1048576".to_owned()),
        (config::BRANCH_KEY_CACHE_TTL_MS, "60000".to_owned()),
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
        (config::HANDS_IMAGE_CATALOG, catalog_json()),
        (config::HANDS_PUBLIC_INTERNET_EGRESS, "true".to_owned()),
        (
            config::CONTENT_KMS_KEY_ARN,
            "arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"
                .to_owned(),
        ),
        (config::MAX_JSON_BODY_BYTES, "65536".to_owned()),
        (config::MAX_PAGE_ITEMS, "100".to_owned()),
        (config::MAX_PAGE_BYTES, "1048576".to_owned()),
    ])
}

fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, RegionalHttpConfigError> {
    Config::read(&|name: &str| vars.get(name).cloned())
}

#[test]
fn a_complete_environment_is_accepted() {
    let admitted = read(&complete()).expect("complete environment");
    assert_eq!(admitted.port, 8_080);
    assert_eq!(admitted.work_table, "aex-dev-regional-work");
    assert_eq!(admitted.limits().json_body_bytes, 65_536);
    assert_eq!(EDGE_COUNT, 1);
}

#[test]
fn every_required_variable_is_required() {
    for name in config::REQUIRED {
        let mut vars = complete();
        vars.remove(name);
        assert!(
            matches!(read(&vars), Err(RegionalHttpConfigError::Missing { name: missing }) if missing == name),
            "{name}"
        );
    }
}

#[test]
fn forbidden_queue_bindings_are_rejected() {
    for (name, _) in config::FORBIDDEN {
        let mut vars = complete();
        vars.insert(name, "bound".to_owned());
        assert!(
            matches!(read(&vars), Err(RegionalHttpConfigError::Forbidden { name: reported, .. }) if reported == name)
        );
    }
}

#[test]
fn the_capability_manifest_matches_the_resolved_configuration() {
    let admitted = read(&complete()).expect("complete environment");
    let manifest = capability::manifest().expect("manifest");
    assert!(manifest.capabilities.contains(WorkClaim::ID));
    assert!(manifest.capabilities.contains(SessionOperationInvoke::ID));
    assert!(manifest.capabilities.contains(ContentEncrypt::ID));
    assert!(manifest.capabilities.contains(SecretPlaintextAdmission::ID));
    capability::admit(&admitted).expect("matching capability bindings");

    let mut extra = capability::resolved(&admitted);
    extra
        .values
        .insert("AEX_UNKNOWN".to_owned(), "bound".to_owned());
    assert!(matches!(
        aex_regional_http::capability::admit(&manifest, &extra),
        Err(CompositionError::UndeclaredKey(_))
    ));
}

#[test]
fn the_session_owner_contains_only_finite_regional_routes() {
    let owned = RouteOwner::SessionApi.routes();
    assert!(!owned.is_empty());
    for id in &owned {
        let descriptor = route(*id);
        assert_eq!(descriptor.plane, Plane::Regional, "{id}");
        assert!(
            matches!(
                descriptor.transport,
                TransportKind::Unary | TransportKind::Binary
            ),
            "{id}"
        );
    }
    assert_eq!(
        route_owner(RouteId::ProviderCredentialRegister),
        Some(RouteOwner::SessionApi)
    );
}

#[test]
fn route_groups_reproduce_the_owned_set() {
    let mut expected: Vec<RouteId> = RouteGroup::ALL
        .iter()
        .flat_map(|group| RouteOwner::SessionApi.routes_in(*group))
        .collect();
    expected.sort_unstable_by_key(|id| *id as usize);
    assert_eq!(expected, RouteOwner::SessionApi.routes());
}
