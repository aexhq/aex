//! Black-box requirements for the regional edge primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use aex_internal_contracts::assertion::{
    AssertionAudience, AuthorizationAssertion, MAX_LIFETIME_MS,
};
use aex_internal_contracts::{Epoch, SchemaVersion};
use aex_regional_http::assertion::{
    AssertionSource, AuthFailure, KeyVerifier, PresentedCredential, ProjectedEpochs,
    SignedAssertion, VerifyingAssertionCache, verify,
};
use aex_regional_http::capability::{
    CapabilityBinding, CompositionManifest, DeployableId, ResolvedConfig, admit,
};
use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::cursor::{
    CursorBinding, CursorError, CursorKey, CursorKeyRing, Order, SNAPSHOT_MILLIS, SnapshotToken,
    SortTuple, decode, encode,
};
use aex_regional_http::envelope::{ENVELOPE_BYTES, EnvelopeError, check_content_type, check_size};
use aex_regional_http::error::{EdgeError, IntoWireError};
use aex_regional_http::idempotency::{IdentityContext, identity, operation_id};
use aex_regional_http::limits::{BodyLimits, LimitError};
use aex_regional_http::page::{PageError, Paginator};
use aex_regional_http::stream::{
    Frame, FrameSink, FrameSplitError, FrameWriter, RotateReason, split_records,
};
use aex_wire::error::{ErrorCode, PrecedenceStage};
use aex_wire::idempotency::{PrincipalKind, PrincipalScope};
use aex_wire::ids::{
    OperationId, OrganizationId, PrefixedId, SessionId, UserId, Uuid7, WorkspaceId,
};
use aex_wire::routes::{BodyClass, RouteId};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{HttpMethod, Region, RequestId, Timestamp};
use async_trait::async_trait;
use http::{HeaderMap, HeaderValue};
use proptest::prelude::*;
use serde_json::json;
use time::OffsetDateTime;

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

fn assertion(expires_at: i64) -> AuthorizationAssertion {
    AuthorizationAssertion {
        schema_version: SchemaVersion::V1,
        principal: PrincipalScope::Account {
            user: user(1),
            organization: Some(organization(1)),
        },
        principal_kind: PrincipalKind::Account,
        workspace: Some(workspace(1)),
        organization: organization(1),
        region: Region::EuWest1,
        scopes: ScopeSet::empty(),
        key_epoch: Epoch(4),
        account_epoch: Epoch(5),
        revocation_epoch: Epoch(6),
        issued_at: stamp(1_000),
        expires_at: stamp(expires_at),
        audience: AssertionAudience::RegionalSession,
    }
}

#[derive(Clone)]
struct TestVerifier;

impl KeyVerifier for TestVerifier {
    fn verify(&self, key_id: &str, _message: &[u8], signature: &[u8]) -> bool {
        key_id == "known" && signature == b"valid"
    }
}

fn signed(credential: &PresentedCredential, expires_at: i64) -> SignedAssertion {
    SignedAssertion::new(
        assertion(expires_at),
        "known",
        credential.binding(),
        b"valid".to_vec(),
    )
    .expect("signed fixture")
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
fn assertions_expire_to_the_millisecond_and_bind_every_authority_fact() {
    let credential = PresentedCredential::new(b"aex_wk_fixture".to_vec()).expect("credential");
    let signed = signed(&credential, 1_000 + MAX_LIFETIME_MS);
    let projected = ProjectedEpochs {
        key: 4,
        account: 5,
        revocation: 6,
    };
    assert!(
        verify(
            &TestVerifier,
            &signed,
            &credential,
            AssertionAudience::RegionalSession,
            Region::EuWest1,
            projected,
            stamp(1_000 + MAX_LIFETIME_MS - 1),
        )
        .is_ok()
    );
    assert_eq!(
        verify(
            &TestVerifier,
            &signed,
            &credential,
            AssertionAudience::RegionalSession,
            Region::EuWest1,
            projected,
            stamp(1_000 + MAX_LIFETIME_MS),
        ),
        Err(AuthFailure::Expired)
    );
    assert_eq!(
        verify(
            &TestVerifier,
            &signed,
            &credential,
            AssertionAudience::RegionalSecret,
            Region::EuWest1,
            projected,
            stamp(2_000),
        ),
        Err(AuthFailure::Audience)
    );
    assert_eq!(
        verify(
            &TestVerifier,
            &signed,
            &credential,
            AssertionAudience::RegionalSession,
            Region::UsEast1,
            projected,
            stamp(2_000),
        ),
        Err(AuthFailure::Region)
    );
    assert_eq!(
        verify(
            &TestVerifier,
            &signed,
            &credential,
            AssertionAudience::RegionalSession,
            Region::EuWest1,
            ProjectedEpochs {
                key: 5,
                ..projected
            },
            stamp(2_000),
        ),
        Err(AuthFailure::EpochRollback)
    );
    let other = PresentedCredential::new(b"aex_wk_other".to_vec()).expect("credential");
    assert_eq!(
        verify(
            &TestVerifier,
            &signed,
            &other,
            AssertionAudience::RegionalSession,
            Region::EuWest1,
            projected,
            stamp(2_000),
        ),
        Err(AuthFailure::CredentialBinding)
    );
}

#[derive(Clone)]
struct CountingSource {
    calls: Arc<AtomicUsize>,
    assertion: SignedAssertion,
}

#[async_trait]
impl AssertionSource for CountingSource {
    async fn obtain(
        &self,
        _credential: &PresentedCredential,
    ) -> Result<SignedAssertion, AuthFailure> {
        self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        tokio::task::yield_now().await;
        Ok(self.assertion.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assertion_refresh_is_single_flight_for_one_credential() {
    let credential = PresentedCredential::new(b"aex_wk_fixture".to_vec()).expect("credential");
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = Arc::new(
        VerifyingAssertionCache::new(
            CountingSource {
                calls: Arc::clone(&calls),
                assertion: signed(&credential, 31_000),
            },
            TestVerifier,
            AssertionAudience::RegionalSession,
            Region::EuWest1,
            4_096,
        )
        .expect("cache"),
    );
    let tasks = (0..100)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let credential = credential.clone();
            tokio::spawn(async move {
                cache
                    .resolve(
                        &credential,
                        ProjectedEpochs {
                            key: 4,
                            account: 5,
                            revocation: 6,
                        },
                        stamp(2_000),
                    )
                    .await
            })
        })
        .collect::<Vec<_>>();
    for task in tasks {
        task.await.expect("task").expect("authorization");
    }
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
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
        .send(Frame::Cursor {
            cursor: "cur_first".into(),
            at: stamp(1),
        })
        .await
        .expect("sent");
    assert_eq!(writer.sent().as_deref(), Some("cur_first"));
    writer.sink_mut().fail_flush = true;
    assert!(
        writer
            .send(Frame::Rotate {
                cursor: "cur_second".into(),
                reason: RotateReason::Draining,
                retryable: true,
                error: None
            })
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

#[test]
fn record_frames_split_at_two_hundred_without_dropping() {
    let records = (0..201)
        .map(|value| json!({"value": value}))
        .collect::<Vec<_>>();
    let cursors = (0..201)
        .map(|value| format!("cur_{value}"))
        .collect::<Vec<_>>();
    let frames = split_records(records.clone(), &cursors).expect("split");
    let flattened = frames
        .iter()
        .flat_map(|frame| match frame {
            Frame::Records { records, .. } => records.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2);
    assert_eq!(flattened, records);
    assert_eq!(
        split_records(
            vec![json!({"body": "x".repeat(1024 * 1024)})],
            &["cur_1".into()]
        ),
        Err(FrameSplitError::RecordTooLarge)
    );
}
