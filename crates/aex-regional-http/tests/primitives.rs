//! Black-box requirements for the regional edge primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use aex_control_domain::epoch::{Epoch as ControlEpoch, EpochSubjectKind};
use aex_identity_domain::assertion::{
    ASSERTION_MAX_LIFETIME_MS, AssertedAccountState, Assertion, AssertionClaims, Audience,
    EpochSlot, EpochSlots, KeyId, LocalSigner, Plane as AssertionPlane, PrincipalKind,
    VerificationKey, VerificationKeySet, issue,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::assertion::{
    AssertionSource, AuthFailure, CredentialFloors, PresentedCredential, VerifyingAssertionCache,
    verify,
};
use aex_regional_http::capability::{
    CapabilityBinding, CompositionManifest, DeployableId, ResolvedConfig, admit,
};
use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::cursor::{
    CursorBinding, CursorError, CursorKey, CursorKeyRing, CursorRequestBinding, Order,
    SNAPSHOT_MILLIS, SnapshotToken, SortTuple, decode, decode_resume, decode_state_resume, encode,
    encode_state,
};
use aex_regional_http::envelope::{ENVELOPE_BYTES, EnvelopeError, check_content_type, check_size};
use aex_regional_http::error::{EdgeError, IntoWireError};
use aex_regional_http::idempotency::{IdentityContext, identity, operation_id};
use aex_regional_http::limits::{BodyLimits, LimitError};
use aex_regional_http::page::{PageError, Paginator};
use aex_regional_http::stream::{
    Frame, FrameSink, FrameSplitError, FrameWriter, RotateReason, split_records,
};
use aex_wire::canonical::CanonicalJson;
use aex_wire::cursor::Cursor as WireCursor;
use aex_wire::error::{ErrorCode, PrecedenceStage};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{
    ObservationId, OperationId, OrganizationId, PrefixedId, SessionId, UserId, Uuid7, WorkspaceId,
};
use aex_wire::models::{
    Observation, ObservationCoverage, ObservationFrameCursor, ObservationFrameRecords,
    ObservationFrameRotate, ObservationSignal,
};
use aex_wire::routes::{BodyClass, Plane, RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{DecimalU128, HttpMethod, Region, RequestId, Timestamp};
use async_trait::async_trait;
use base64::Engine as _;
use http::{HeaderMap, HeaderValue};
use http_body_util::BodyExt as _;
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use tower::ServiceExt as _;

fn workspace(seed: u8) -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [seed; 10]))
}

fn session(seed: u8) -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(2, [seed; 10]))
}

fn organization(seed: u8) -> OrganizationId {
    OrganizationId::from_uuid7(Uuid7::compose(3, [seed; 10]))
}

fn user(seed: u8) -> UserId {
    UserId::from_uuid7(Uuid7::compose(4, [seed; 10]))
}

fn request_context() -> RequestContext {
    RequestContext {
        request_id: RequestId::parse("request-fixture").expect("request id"),
        route: RouteId::SessionCreate,
        auth: RegionalAuthorization {
            principal: PrincipalScope::Account {
                user: user(1),
                organization: Some(organization(1)),
            },
            credential_binding: [7; 32],
            organization_id: organization(1),
            workspace_id: workspace(1),
            placement: Region::EuWest1,
            scopes: ScopeSet::empty(),
            account_state: AccountState::Active,
            epochs: AuthorizationEpochs::default(),
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(30),
        },
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            query_page_items: 100,
            query_page_bytes: 8 * 1024 * 1024,
        },
        operation_id: None,
        idempotency: None,
        if_match: None,
        received_at: OffsetDateTime::UNIX_EPOCH,
    }
}

/// A syntactically complete workspace API key, and the credential it becomes.
fn fixture_credential(seed: u8) -> PresentedCredential {
    let uuid = aex_wire::Uuid7::compose(1_754_051_696_789, [seed; 10]);
    let suffix = String::from_utf8(uuid.encode_suffix().to_vec()).expect("Crockford is ASCII");
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([seed; 32]);
    let token = format!("aex_wk_{}_{suffix}_{secret}", Region::EuWest1.code());
    PresentedCredential::new(token.into_bytes()).expect("a workspace key is a credential")
}

fn signer() -> LocalSigner {
    LocalSigner::new(
        KeyId::new(uuid::Uuid::from_u128(0x515)),
        &zeroize::Zeroizing::new([13_u8; 32]),
    )
}

fn anchors() -> VerificationKeySet {
    VerificationKeySet::new(vec![VerificationKey {
        kid: KeyId::new(uuid::Uuid::from_u128(0x515)),
        public_key: signer().public_key(),
        not_after_ms: u64::MAX,
    }])
    .expect("one key")
}

fn audience(service: AssertionAudience) -> Audience {
    Audience {
        plane: AssertionPlane::Dev,
        region: Region::EuWest1,
        service,
    }
}

fn raw_id<I: aex_wire::ids::PrefixedId>(id: I) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*id.uuid7().as_bytes())
}

fn signed(credential: &PresentedCredential, lifetime_ms: u64) -> Assertion {
    let claims = AssertionClaims {
        issued_at_ms: 1_000,
        expires_at_ms: 1_000 + lifetime_ms,
        audience: audience(AssertionAudience::RegionalSession),
        principal_kind: PrincipalKind::WorkspaceKey,
        principal_id: credential.key_id_raw(),
        credential_binding: credential.expected_binding(),
        organization_id: raw_id(organization(1)),
        workspace_id: raw_id(workspace(1)),
        workspace_region: Region::EuWest1,
        account_state: AssertedAccountState::Active,
        scopes: aex_control_domain::ScopeSet::EMPTY,
        epochs: EpochSlots::new(&[EpochSlot {
            kind: EpochSubjectKind::Key,
            id: credential.key_id_raw(),
            epoch: ControlEpoch::new(4),
        }])
        .expect("one subject"),
    };
    issue(&signer(), &claims).expect("a 30-second envelope")
}

fn floors(credential: &PresentedCredential, key: u64) -> CredentialFloors {
    CredentialFloors::new(ControlEpoch::new(key), credential.key_id_raw())
}

fn binding() -> CursorBinding {
    CursorBinding {
        route: RouteId::SessionsList,
        principal_scope: [1; 32],
        region: Region::EuWest1,
        workspace_id: workspace(1),
        session_id: Some(session(1)),
        query_hash: [2; 32],
        order: Order::Descending,
        snapshot: SnapshotToken::new("authority-revision-7").expect("snapshot"),
    }
}

fn key(id: &str, byte: u8) -> CursorKey {
    CursorKey::new(id, vec![byte; 32]).expect("strong key")
}

fn stamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("timestamp")
}

#[test]
fn cursor_round_trips_and_rotation_accepts_the_overlap_key() {
    let old = key("old", 3);
    let token = encode(
        &old,
        &binding(),
        &SortTuple::new(vec!["2026-08-01".into(), "ses_1".into()]).expect("tuple"),
        stamp(10),
    )
    .expect("encode");
    let ring = CursorKeyRing::new(key("current", 4), vec![old]).expect("ring");
    let decoded = decode(&ring, &token, &binding(), stamp(20)).expect("overlap verifies");
    assert_eq!(decoded.parts(), &["2026-08-01", "ses_1"]);
}

#[test]
fn a_stream_reconnect_recovers_its_authenticated_snapshot() {
    let signing = key("current", 5);
    let original = binding();
    let token = encode(
        &signing,
        &original,
        &SortTuple::new(vec!["last-row".into()]).expect("tuple"),
        stamp(10),
    )
    .expect("encode");
    let ring = CursorKeyRing::new(signing, vec![]).expect("ring");
    let request = CursorRequestBinding::from(&original);
    let resumed = decode_resume(&ring, &token, &request, stamp(20)).expect("resume");
    assert_eq!(resumed.snapshot.as_str(), "authority-revision-7");
    assert_eq!(resumed.tuple.parts(), &["last-row"]);

    let mut wrong = request;
    wrong.query_hash = [99; 32];
    assert_eq!(
        decode_resume(&ring, &token, &wrong, stamp(20)),
        Err(CursorError::NotBound)
    );
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TestResume {
    bucket: String,
    positions: Vec<String>,
}

#[test]
fn a_typed_cursor_round_trips_state_without_accepting_a_tuple_decoder() {
    let signing = key("current", 6);
    let original = binding();
    let state = TestResume {
        bucket: "2026-08-01T09".to_owned(),
        positions: vec!["logs:0:row-7".to_owned()],
    };
    let token = encode_state(&signing, &original, &state, stamp(10)).expect("state encodes");
    let ring = CursorKeyRing::new(signing, vec![]).expect("ring");
    let request = CursorRequestBinding::from(&original);
    let resumed = decode_state_resume::<TestResume>(&ring, &token, &request, stamp(20))
        .expect("typed state resumes");
    assert_eq!(resumed.snapshot.as_str(), "authority-revision-7");
    assert_eq!(resumed.state, state);
    assert_eq!(
        decode_resume(&ring, &token, &request, stamp(20)),
        Err(CursorError::NotBound)
    );
}

#[test]
fn typed_cursor_state_cannot_exceed_the_public_envelope_bound() {
    let signing = key("current", 8);
    let state = TestResume {
        bucket: "2026-08-01T09".to_owned(),
        positions: vec!["x".repeat(aex_wire::cursor::Cursor::MAX_BYTES); 2],
    };
    assert_eq!(
        encode_state(&signing, &binding(), &state, stamp(10)),
        Err(CursorError::Malformed)
    );
}

#[test]
fn every_cursor_binding_field_is_authenticated() {
    let key = key("current", 9);
    let token = encode(
        &key,
        &binding(),
        &SortTuple::new(vec!["row".into()]).expect("tuple"),
        stamp(0),
    )
    .expect("encode");
    let ring = CursorKeyRing::new(key, vec![]).expect("ring");
    let mut variants = Vec::new();
    let mut changed = binding();
    changed.route = RouteId::RegionalOperationsList;
    variants.push(changed);
    let mut changed = binding();
    changed.principal_scope = [8; 32];
    variants.push(changed);
    let mut changed = binding();
    changed.region = Region::UsEast1;
    variants.push(changed);
    let mut changed = binding();
    changed.workspace_id = workspace(2);
    variants.push(changed);
    let mut changed = binding();
    changed.session_id = Some(session(2));
    variants.push(changed);
    let mut changed = binding();
    changed.query_hash = [9; 32];
    variants.push(changed);
    let mut changed = binding();
    changed.order = Order::Ascending;
    variants.push(changed);
    let mut changed = binding();
    changed.snapshot = SnapshotToken::new("authority-revision-8").expect("snapshot");
    variants.push(changed);
    for changed in variants {
        assert_eq!(
            decode(&ring, &token, &changed, stamp(0)),
            Err(CursorError::NotBound)
        );
    }
}

#[test]
fn cursor_expiry_and_mac_failure_fail_closed() {
    let key = key("current", 7);
    let token = encode(
        &key,
        &binding(),
        &SortTuple::new(vec!["row".into()]).expect("tuple"),
        stamp(100),
    )
    .expect("encode");
    let ring = CursorKeyRing::new(key, vec![]).expect("ring");
    assert!(decode(&ring, &token, &binding(), stamp(100 + SNAPSHOT_MILLIS)).is_ok());
    assert_eq!(
        decode(&ring, &token, &binding(), stamp(101 + SNAPSHOT_MILLIS)),
        Err(CursorError::Expired)
    );
    let mut tampered = token.as_str().to_owned().into_bytes();
    let last = tampered.last_mut().expect("body");
    *last = if *last == b'A' { b'B' } else { b'A' };
    let tampered = aex_wire::cursor::Cursor::parse(std::str::from_utf8(&tampered).expect("utf8"))
        .expect("envelope grammar");
    assert!(matches!(
        decode(&ring, &tampered, &binding(), stamp(100)),
        Err(CursorError::NotBound | CursorError::Malformed)
    ));
}

proptest! {
    #[test]
    fn cursor_sort_tuple_round_trips(parts in prop::collection::vec("[a-z0-9_-]{1,32}", 1..=4)) {
        let key = key("current", 11);
        let tuple = SortTuple::new(parts.clone()).expect("bounded tuple");
        let token = encode(&key, &binding(), &tuple, stamp(100)).expect("encode");
        let ring = CursorKeyRing::new(key, vec![]).expect("ring");
        let decoded = decode(&ring, &token, &binding(), stamp(100)).expect("decode");
        prop_assert_eq!(decoded.parts(), parts.as_slice());
    }
}

#[test]
fn idempotency_uses_canonical_bytes_and_separates_scope() {
    let left = aex_wire::canonical::to_jcs_bytes(&json!({"b": 2, "a": 1})).expect("jcs");
    let right = aex_wire::canonical::to_jcs_bytes(&json!({"a": 1, "b": 2})).expect("jcs");
    assert_eq!(left, right);
    let first = IdentityContext {
        principal: "key_1",
        organization: "org_1",
        workspace: workspace(1),
        route: RouteId::SessionCreate,
        method: HttpMethod::Post,
    };
    let second = IdentityContext {
        workspace: workspace(2),
        ..first
    };
    let key = aex_wire::idempotency::IdempotencyKey::parse("request-1").expect("key");
    assert_eq!(
        identity(&first, &key, &left).intent,
        identity(&first, &key, &right).intent
    );
    assert_ne!(
        identity(&first, &key, &left).scope,
        identity(&second, &key, &left).scope
    );
    assert_ne!(
        identity(&first, &key, &left).intent,
        identity(&first, &key, b"{}").intent
    );
}

#[test]
fn operation_header_is_the_only_operation_identity_carrier() {
    let expected = OperationId::from_uuid7(Uuid7::compose(3, [4; 10]));
    let mut headers = HeaderMap::new();
    headers.insert(
        "Aex-Operation-Id",
        HeaderValue::from_str(&expected.to_string()).expect("header"),
    );
    assert_eq!(operation_id(&headers).expect("operation id"), expected);
    headers.insert("Aex-Operation-Id", HeaderValue::from_static("ses_wrong"));
    assert!(operation_id(&headers).is_err());
}

#[test]
fn runtime_capability_admission_fails_closed() {
    let manifest = CompositionManifest {
        deployable: DeployableId::new("regional-session-api").expect("id"),
        capabilities: BTreeSet::from(["content.encrypt"]),
        bindings: vec![CapabilityBinding::arn(
            "AEX_CONTENT_KMS_KEY_ARN",
            "content.encrypt",
        )],
    };
    let valid = ResolvedConfig {
        deployable: "regional-session-api".into(),
        plane: "dev".into(),
        region: Region::EuWest1,
        account_id: "522921482290".into(),
        values: BTreeMap::from([(
            "AEX_CONTENT_KMS_KEY_ARN".into(),
            "arn:aws:kms:eu-west-1:522921482290:key/example".into(),
        )]),
    };
    assert!(admit(&manifest, &valid).is_ok());
    let mut extra = valid.clone();
    extra
        .values
        .insert("AEX_SECRET_KMS_KEY_ARN".into(), "secret".into());
    assert!(admit(&manifest, &extra).is_err());
    let mut wrong_deployable = valid.clone();
    wrong_deployable.deployable = "regional-secret-api".into();
    assert!(admit(&manifest, &wrong_deployable).is_err());
    let mut off_plane = valid;
    off_plane.values.insert(
        "AEX_CONTENT_KMS_KEY_ARN".into(),
        "arn:aws:kms:us-east-1:000000000000:key/example".into(),
    );
    assert!(admit(&manifest, &off_plane).is_err());
}

#[test]
fn body_limits_refuse_without_clamping() {
    let limits = BodyLimits::new(10 * 1024 * 1024, 65_536).expect("limits");
    assert!(limits.check_envelope(10 * 1024 * 1024).is_ok());
    assert_eq!(
        limits.check_envelope(10 * 1024 * 1024 + 1),
        Err(LimitError::EnvelopeTooLarge {
            measured: 10 * 1024 * 1024 + 1,
            maximum: 10 * 1024 * 1024
        })
    );
    assert!(limits.check_json(65_535).is_ok());
    assert!(limits.check_json(65_536).is_ok());
    assert_eq!(
        limits.check_json(65_537),
        Err(LimitError::JsonBodyTooLarge {
            measured: 65_537,
            maximum: 65_536
        })
    );
}

#[test]
fn envelope_and_content_type_are_checked_before_dynamic_limits() {
    assert!(check_size(ENVELOPE_BYTES).is_ok());
    assert_eq!(
        check_size(ENVELOPE_BYTES + 1),
        Err(EnvelopeError::TooLarge {
            measured: ENVELOPE_BYTES + 1,
            maximum: ENVELOPE_BYTES,
        })
    );
    assert!(check_content_type(BodyClass::None, None).is_ok());
    assert!(
        check_content_type(
            BodyClass::AexJson,
            Some(&HeaderValue::from_static("application/json; charset=utf-8"))
        )
        .is_ok()
    );
    assert!(
        check_content_type(
            BodyClass::AexJson,
            Some(&HeaderValue::from_static("text/plain"))
        )
        .is_err()
    );
}

#[test]
fn edge_error_mapping_is_total_and_never_echoes_sensitive_input() {
    let cases = [
        (EdgeError::Unauthenticated, ErrorCode::Unauthenticated),
        (
            EdgeError::AuthenticationUnavailable,
            ErrorCode::AuthenticationUnavailable,
        ),
        (EdgeError::Forbidden, ErrorCode::Forbidden),
        (
            EdgeError::WrongWorkspaceRegion,
            ErrorCode::WrongWorkspaceRegion,
        ),
        (EdgeError::InsufficientScope, ErrorCode::InsufficientScope),
        (EdgeError::AccountPaused, ErrorCode::AccountPaused),
        (
            EdgeError::AccountStateUnavailable,
            ErrorCode::AccountStateUnavailable,
        ),
        (
            EdgeError::IdempotencyConflict,
            ErrorCode::IdempotencyConflict,
        ),
        (
            EdgeError::OperationIdempotencyConflict,
            ErrorCode::OperationIdempotencyConflict,
        ),
        (EdgeError::PreconditionFailed, ErrorCode::PreconditionFailed),
        (EdgeError::CommitUnavailable, ErrorCode::UpstreamError),
        (EdgeError::Internal, ErrorCode::InternalError),
    ];
    for (error, expected) in cases {
        let envelope = error.into_wire(&request_context());
        assert_eq!(envelope.error.code.known(), Some(expected));
        let rendered = serde_json::to_string(&envelope).expect("error JSON");
        for secret in [
            "aex_wk_secret",
            "https://signed.example/?X-Amz-Signature=secret",
            "workspaces/private/object-key",
            "prompt bytes",
        ] {
            assert!(!rendered.contains(secret));
        }
    }
}

#[test]
fn edge_precedence_is_the_complete_wire_table() {
    assert_eq!(
        aex_regional_http::router::EDGE_PRECEDENCE,
        PrecedenceStage::ALL
    );
    assert_eq!(
        aex_regional_http::router::EDGE_PRECEDENCE.map(PrecedenceStage::order),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
    );
}

#[test]
fn every_generated_regional_route_has_exactly_one_planned_owner() {
    use aex_regional_http::router::{RouteOwner, route_owner};

    for id in RouteId::ALL {
        if route(*id).plane == Plane::Regional {
            let owner = route_owner(*id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(
                owner.deployable(),
                route(*id).serving_artifact,
                "planned runtime and generated delivery ownership disagree for {id}"
            );
        } else {
            assert_eq!(route_owner(*id), None, "{id}");
        }
    }
    assert_eq!(route_owner(RouteId::SecretPut), Some(RouteOwner::SecretApi));
    assert_eq!(
        route_owner(RouteId::ProviderCredentialRegister),
        Some(RouteOwner::SecretApi)
    );
    assert_eq!(
        route_owner(RouteId::SecretGet),
        Some(RouteOwner::SessionApi)
    );
    assert_eq!(
        route_owner(RouteId::SessionObservationsEventsListen),
        Some(RouteOwner::Stream)
    );
    assert_eq!(
        route_owner(RouteId::SessionObservationsEventsQuery),
        Some(RouteOwner::ObservationApi)
    );
}

#[test]
fn assertions_expire_to_the_millisecond_and_bind_every_authority_fact() {
    let credential = fixture_credential(5);
    let assertion = signed(&credential, ASSERTION_MAX_LIFETIME_MS);
    let session = audience(AssertionAudience::RegionalSession);
    let floor = floors(&credential, 4);

    // Accepted at expiry minus one millisecond and refused at expiry: the bound
    // is exact, and there is no grace period anywhere.
    assert!(
        verify(
            &assertion,
            &anchors(),
            &credential,
            session,
            &floor,
            stamp(1_000 + i64::try_from(ASSERTION_MAX_LIFETIME_MS).expect("small") - 1),
        )
        .is_ok()
    );
    assert_eq!(
        verify(
            &assertion,
            &anchors(),
            &credential,
            session,
            &floor,
            stamp(1_000 + i64::try_from(ASSERTION_MAX_LIFETIME_MS).expect("small")),
        ),
        Err(AuthFailure::Verification(
            aex_identity_domain::assertion::VerifyError::Expired
        ))
    );
}

#[test]
fn every_authority_fact_the_envelope_binds_is_refused_on_its_own() {
    let credential = fixture_credential(5);
    let assertion = signed(&credential, ASSERTION_MAX_LIFETIME_MS);
    let session = audience(AssertionAudience::RegionalSession);
    let floor = floors(&credential, 4);

    for (inputs, expected) in [
        (
            (
                audience(AssertionAudience::RegionalSecret),
                floor,
                credential.clone(),
            ),
            aex_identity_domain::assertion::VerifyError::AudienceMismatch,
        ),
        (
            (
                Audience {
                    region: Region::UsEast1,
                    ..session
                },
                floor,
                credential.clone(),
            ),
            aex_identity_domain::assertion::VerifyError::AudienceMismatch,
        ),
        (
            (
                Audience {
                    plane: AssertionPlane::Prd,
                    ..session
                },
                floor,
                credential.clone(),
            ),
            aex_identity_domain::assertion::VerifyError::AudienceMismatch,
        ),
        (
            (session, floors(&credential, 5), credential.clone()),
            aex_identity_domain::assertion::VerifyError::EpochStale {
                kind: EpochSubjectKind::Key,
                id: credential.key_id_raw(),
                claimed: 4,
                projected: 5,
            },
        ),
        (
            (session, floor, fixture_credential(6)),
            aex_identity_domain::assertion::VerifyError::CredentialBindingMismatch,
        ),
    ] {
        let (audience, floor, presented) = inputs;
        assert_eq!(
            verify(
                &assertion,
                &anchors(),
                &presented,
                audience,
                &floor,
                stamp(2_000)
            ),
            Err(AuthFailure::Verification(expected)),
            "{expected:?}"
        );
    }

    // An anchor set that does not hold the signing identity verifies nothing.
    let empty = VerificationKeySet::new(Vec::new()).expect("an empty set");
    assert_eq!(
        verify(
            &assertion,
            &empty,
            &credential,
            session,
            &floor,
            stamp(2_000)
        ),
        Err(AuthFailure::Verification(
            aex_identity_domain::assertion::VerifyError::UnknownKid
        ))
    );
}

#[derive(Clone)]
struct CountingSource {
    calls: Arc<AtomicUsize>,
    assertion: Assertion,
}

#[async_trait]
impl AssertionSource for CountingSource {
    async fn obtain(&self, _credential: &PresentedCredential) -> Result<Assertion, AuthFailure> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        tokio::task::yield_now().await;
        Ok(self.assertion)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assertion_refresh_is_single_flight_for_one_credential() {
    let credential = fixture_credential(5);
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = Arc::new(
        VerifyingAssertionCache::new(
            CountingSource {
                calls: Arc::clone(&calls),
                assertion: signed(&credential, ASSERTION_MAX_LIFETIME_MS),
            },
            anchors(),
            audience(AssertionAudience::RegionalSession),
            4_096,
        )
        .expect("cache"),
    );
    let floor = floors(&credential, 4);
    let tasks = (0..100)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let credential = credential.clone();
            tokio::spawn(async move { cache.resolve(&credential, &floor, stamp(2_000)).await })
        })
        .collect::<Vec<_>>();
    for task in tasks {
        task.await.expect("task").expect("authorization");
    }
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
}

#[tokio::test]
async fn a_cached_assertion_is_dropped_the_instant_its_credential_floor_moves() {
    let credential = fixture_credential(5);
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = VerifyingAssertionCache::new(
        CountingSource {
            calls: Arc::clone(&calls),
            assertion: signed(&credential, ASSERTION_MAX_LIFETIME_MS),
        },
        anchors(),
        audience(AssertionAudience::RegionalSession),
        4_096,
    )
    .expect("cache");

    cache
        .resolve(&credential, &floors(&credential, 4), stamp(2_000))
        .await
        .expect("a current assertion");
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    // Served from the cache: no second exchange.
    cache
        .resolve(&credential, &floors(&credential, 4), stamp(2_100))
        .await
        .expect("the cached assertion");
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

    // A revocation published a moment ago beats the cached entry, and the
    // refreshed answer is refused too because it claims the same epoch.
    assert!(
        cache
            .resolve(&credential, &floors(&credential, 5), stamp(2_200))
            .await
            .is_err()
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
}

#[test]
fn paginator_splits_before_the_byte_bound_and_keeps_continuity() {
    let items = vec!["a".repeat(8), "b".repeat(8), "c".repeat(8)];
    let page = Paginator::new(100, 30)
        .expect("bounds")
        .page(&items, 0)
        .expect("page");
    assert!(!page.items.is_empty());
    assert!(page.items.len() < items.len());
    let next = page.next_index.expect("continuation");
    assert_eq!(page.items, items[..next]);
    let rest = Paginator::new(100, 30)
        .expect("bounds")
        .page(&items, next)
        .expect("page");
    let recombined = page.items.into_iter().chain(rest.items).collect::<Vec<_>>();
    assert_eq!(recombined, items);
    assert_eq!(
        Paginator::new(1_001, 1),
        Err(PageError::ItemLimitTooLarge {
            found: 1_001,
            maximum: 1_000
        })
    );
}

#[derive(Default)]
struct ScriptedSink {
    writes: Vec<Vec<u8>>,
    fail_flush: bool,
}

#[async_trait]
impl FrameSink for ScriptedSink {
    type Error = &'static str;

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.writes.push(bytes.to_vec());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        if self.fail_flush {
            Err("flush")
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn sent_cursor_advances_only_after_a_whole_flushed_frame() {
    let mut writer = FrameWriter::new(ScriptedSink::default());
    writer
        .send(Frame::Cursor(ObservationFrameCursor {
            coverage: coverage(1),
            cursor: WireCursor::parse("cur_first").expect("cursor"),
        }))
        .await
        .expect("sent");
    assert_eq!(writer.sent().as_deref(), Some("cur_first"));
    writer
        .send(Frame::Records(ObservationFrameRecords {
            items: vec![observation(1, &json!({"value": 1}))],
        }))
        .await
        .expect("records sent");
    assert_eq!(writer.sent().as_deref(), Some("cur_first"));
    writer.sink_mut().fail_flush = true;
    assert!(
        writer
            .send(Frame::Rotate(ObservationFrameRotate {
                cursor: Some(WireCursor::parse("cur_second").expect("cursor")),
                reason: RotateReason::ServerRotating,
                retryable: true,
                error: None
            }))
            .await
            .is_err()
    );
    assert_eq!(writer.sent().as_deref(), Some("cur_first"));
    assert!(
        writer
            .sink()
            .writes
            .iter()
            .all(|write| write.ends_with(b"\n"))
    );
}

fn coverage(value: u128) -> ObservationCoverage {
    ObservationCoverage {
        accepted: DecimalU128::new(value),
        caught_up: true,
        complete: true,
        earliest_replay: DecimalU128::ZERO,
        indexed: DecimalU128::new(value),
        missing_intervals: Vec::new(),
        snapshot: DecimalU128::new(value),
        unbounded_gaps: Vec::new(),
    }
}

fn observation(value: u64, body: &serde_json::Value) -> Observation {
    Observation {
        accepted_at: stamp(i64::try_from(value).expect("fixture millis")),
        body: CanonicalJson::from_value(body).expect("canonical fixture"),
        id: ObservationId::from_uuid7(Uuid7::compose(value, [1; 10])),
        observed_at: stamp(i64::try_from(value).expect("fixture millis")),
        run_id: None,
        sequence: DecimalU128::new(u128::from(value)),
        session_id: None,
        signal: ObservationSignal::Logs,
        span_id: None,
        trace_id: None,
        workspace_id: workspace(1),
    }
}

#[test]
fn record_frames_split_at_two_hundred_without_dropping() {
    let records = (0..201)
        .map(|value| observation(value, &json!({"value": value})))
        .collect::<Vec<_>>();
    let frames = split_records(records.clone()).expect("split");
    let flattened = frames
        .iter()
        .flat_map(|frame| match frame {
            Frame::Records(ObservationFrameRecords { items }) => items.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2);
    assert_eq!(flattened, records);
    for frame in &frames {
        let encoded = serde_json::to_vec(frame).expect("generated frame encodes");
        let decoded: Frame = serde_json::from_slice(&encoded).expect("generated frame decodes");
        assert_eq!(&decoded, frame);
        let value = serde_json::to_value(frame).expect("frame value");
        assert!(value.get("cursor").is_none());
        assert!(value.get("items").is_some());
    }
    assert_eq!(
        split_records(vec![observation(
            1,
            &json!({"body": "x".repeat(1024 * 1024)})
        )]),
        Err(FrameSplitError::RecordTooLarge)
    );
}

#[tokio::test]
async fn health_and_readiness_paths_are_internal_no_store_and_fail_closed() {
    let healthy = aex_regional_http::health::router(aex_regional_http::health::Readiness::ready(
        "sha256:release",
    ));
    let response = healthy
        .oneshot(
            http::Request::builder()
                .uri("/internal/healthz")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");

    let not_ready = aex_regional_http::health::router(
        aex_regional_http::health::Readiness::not_ready(
            "sha256:release",
            ["cursor_key_ring", "authority_reader"],
        )
        .expect("readiness"),
    );
    let response = not_ready
        .oneshot(
            http::Request::builder()
                .uri("/internal/readyz")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 503);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(value["status"], "not_ready");
    assert_eq!(value["unavailable"].as_array().expect("array").len(), 2);
}
