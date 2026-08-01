//! Black-box requirements for the concrete edge adapters: the trust anchors, the
//! cursor signing ring, the `central-authz` exchange payloads, the regional
//! authorization projection, and the whole admission chain composed over them.
//!
//! Nothing here reaches AWS. Every adapter is split so the decision is a pure
//! function of bytes and the client call is the only thing left un-exercised,
//! which is exactly the part a live companion owns.

use std::sync::Mutex;

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_identity_domain::assertion::{
    AssertedAccountState, Assertion, AssertionClaims, Audience, EpochSlot, EpochSlots, KeyId,
    LocalSigner, Plane as AssertionPlane, PrincipalKind, VerificationKeySet, issue,
};
use aex_internal_contracts::assertion::{
    AssertionAudience, AssertionRefusal, AssertionResponse, ResolveWorkspaceKey,
};
use aex_regional_http::assertion::{
    AssertionSource, AuthFailure, PresentedCredential, ProjectedEpochs,
};
use aex_regional_http::authz::{
    MAX_PARAMETER_BYTES, MAX_TRUST_ANCHORS, RegionalProjection, TrustError, decode_response,
    issued_assertion, parse_cursor_key_ring, parse_trust_anchors, resolve_request,
};
use aex_regional_http::context::{AccountState, EffectiveLimits};
use aex_regional_http::cursor::{CursorBinding, Order, SnapshotToken, SortTuple, decode, encode};
use aex_regional_http::edge::{
    EdgeBinding, EdgeClock, ProjectedState, ProjectionError, ProjectionReader, RegionalEdge,
};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::AuthorizationProjection;
use aex_session_dynamodb::wire_pending::{FeedFrontier, KeyRevocation, WorkspacePlacement};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IdempotencyKind;
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::routes::{Plane, RouteId, route};
use aex_wire::types::{HttpMethod, Region, RequestId, Timestamp};
use async_trait::async_trait;
use base64::Engine as _;
use http::{HeaderMap, HeaderValue};
use uuid::Uuid;

// --- fixtures -------------------------------------------------------------------

const ANCHOR_PARAM: &str = "/aex/dev/authz/verify-keys";
const CURSOR_PARAM: &str = "/aex/dev/regional/cursor-signing-key";
const KID: Uuid = Uuid::from_u128(0x2026_080a);
const NOW_MS: i64 = 1_754_051_698_000;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sample<I: PrefixedId>(seed: u8) -> I {
    I::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]))
}

fn workspace() -> WorkspaceId {
    sample::<WorkspaceId>(2)
}

fn organization() -> OrganizationId {
    sample::<OrganizationId>(4)
}

fn raw<I: PrefixedId>(id: I) -> Uuid {
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("a representable instant")
}

/// A syntactically complete workspace API key, and the identity it embeds.
fn workspace_key(region: Region, seed: u8) -> (String, ApiKeyId) {
    let uuid = Uuid7::compose(1_754_051_696_789, [seed; 10]);
    let suffix = String::from_utf8(uuid.encode_suffix().to_vec()).expect("Crockford is ASCII");
    let token = format!("aex_wk_{}_{suffix}_{}", region.code(), b64(&[seed; 32]));
    (token, ApiKeyId::from_uuid7(uuid))
}

fn credential_pair(region: Region, seed: u8) -> (PresentedCredential, ApiKeyId) {
    let (token, key) = workspace_key(region, seed);
    (
        PresentedCredential::new(token.into_bytes()).expect("a workspace key is a credential"),
        key,
    )
}

fn signer() -> LocalSigner {
    LocalSigner::new(KeyId::new(KID), &zeroize::Zeroizing::new([13_u8; 32]))
}

fn anchor_document(key_id: Uuid, public_key: &[u8], not_after_ms: u64) -> String {
    format!(
        r#"{{"schemaVersion":1,"keys":[{{"keyId":"{key_id}","publicKey":"{}","notAfterMs":{not_after_ms}}}]}}"#,
        b64(public_key)
    )
}

fn keys() -> VerificationKeySet {
    parse_trust_anchors(
        ANCHOR_PARAM,
        &anchor_document(KID, &signer().public_key(), u64::MAX),
    )
    .expect("a well-formed anchor document")
}

/// The claims `central-authz` mints for a presented workspace key.
///
/// The epoch slots are exactly the three subjects a regional projection holds —
/// key, workspace and account — because those are the only ones this plane can
/// check a revocation against.
fn claims(
    credential: &PresentedCredential,
    audience: AssertionAudience,
    scopes: aex_control_domain::ScopeSet,
    issued_at_ms: u64,
) -> AssertionClaims {
    AssertionClaims {
        issued_at_ms,
        expires_at_ms: issued_at_ms + 30_000,
        audience: Audience {
            plane: AssertionPlane::Dev,
            region: Region::EuWest1,
            service: audience,
        },
        principal_kind: PrincipalKind::WorkspaceKey,
        principal_id: credential.key_id_raw(),
        credential_binding: credential.expected_binding(),
        organization_id: raw(organization()),
        workspace_id: raw(workspace()),
        workspace_region: Region::EuWest1,
        account_state: AssertedAccountState::Active,
        scopes,
        epochs: EpochSlots::new(&[
            EpochSlot {
                kind: EpochSubjectKind::Key,
                id: credential.key_id_raw(),
                epoch: Epoch::new(10),
            },
            EpochSlot {
                kind: EpochSubjectKind::Workspace,
                id: raw(workspace()),
                epoch: Epoch::new(30),
            },
            EpochSlot {
                kind: EpochSubjectKind::Account,
                id: raw(organization()),
                epoch: Epoch::new(20),
            },
        ])
        .expect("three distinct subjects"),
    }
}

fn envelope(claims: &AssertionClaims) -> Assertion {
    issue(&signer(), claims).expect("a 30-second envelope")
}

fn answer(claims: &AssertionClaims) -> AssertionResponse {
    AssertionResponse::Issued {
        assertion: envelope(claims)
            .to_issued()
            .expect("a fixed-length envelope"),
    }
}

// --- trust anchors ---------------------------------------------------------------

#[test]
fn a_well_formed_anchor_document_yields_exactly_its_declared_keys() {
    let keys = keys();
    assert_eq!(keys.len(), 1);
    assert!(!keys.is_empty());
    assert!(keys.find(KeyId::new(KID), 0).is_some());
    assert!(
        keys.find(KeyId::new(Uuid::from_u128(1)), 0).is_none(),
        "an identity outside the ring must never resolve"
    );
}

#[test]
fn an_anchor_that_has_lapsed_stops_verifying_without_a_redeploy() {
    let keys = parse_trust_anchors(
        ANCHOR_PARAM,
        &anchor_document(KID, &signer().public_key(), 1_000),
    )
    .expect("a well-formed anchor document");
    assert!(keys.find(KeyId::new(KID), 999).is_some());
    assert!(
        keys.find(KeyId::new(KID), 1_000).is_none(),
        "`notAfterMs` is the instant the region stops accepting the key"
    );
}

#[test]
fn an_anchor_document_declaring_no_key_is_refused_rather_than_admitting_nothing() {
    let error = parse_trust_anchors(ANCHOR_PARAM, r#"{"schemaVersion":1,"keys":[]}"#)
        .expect_err("an empty anchor set cannot verify anything");
    assert_eq!(
        error,
        TrustError::NoAnchors {
            name: ANCHOR_PARAM.to_owned()
        }
    );
}

#[test]
fn an_anchor_document_with_an_unknown_member_is_refused() {
    let document = r#"{"schemaVersion":1,"keys":[],"rotateAt":"soon"}"#;
    let error = parse_trust_anchors(ANCHOR_PARAM, document).expect_err("unknown members are typos");
    assert!(
        matches!(error, TrustError::Malformed { kind, .. } if kind == "trust anchor"),
        "{error}"
    );
}

#[test]
fn an_anchor_document_past_the_bound_is_refused() {
    let keys: Vec<String> = (0..=MAX_TRUST_ANCHORS)
        .map(|index| {
            let seed = u8::try_from(index).expect("the bound is far below 255");
            format!(
                r#"{{"keyId":"{}","publicKey":"{}","notAfterMs":1}}"#,
                Uuid::from_u128(u128::try_from(index).expect("small")),
                b64(&[seed; 32])
            )
        })
        .collect();
    let document = format!(r#"{{"schemaVersion":1,"keys":[{}]}}"#, keys.join(","));
    let error = parse_trust_anchors(ANCHOR_PARAM, &document).expect_err("past the bound");
    assert_eq!(
        error,
        TrustError::TooManyAnchors {
            name: ANCHOR_PARAM.to_owned(),
            found: MAX_TRUST_ANCHORS + 1
        }
    );
}

#[test]
fn two_anchors_claiming_one_identity_are_refused() {
    let document = format!(
        r#"{{"schemaVersion":1,"keys":[{{"keyId":"{KID}","publicKey":"{0}","notAfterMs":1}},{{"keyId":"{KID}","publicKey":"{0}","notAfterMs":2}}]}}"#,
        b64(&[1; 32])
    );
    let error = parse_trust_anchors(ANCHOR_PARAM, &document).expect_err("an ambiguous ring");
    assert_eq!(
        error,
        TrustError::DuplicateKeyId {
            name: ANCHOR_PARAM.to_owned(),
            key_id: KID.to_string()
        }
    );
}

#[test]
fn key_material_that_is_not_exactly_an_ed25519_public_key_is_refused() {
    for material in [b64(&[1; 31]), b64(&[1; 33]), "not base64".to_owned()] {
        let document = format!(
            r#"{{"schemaVersion":1,"keys":[{{"keyId":"{KID}","publicKey":"{material}","notAfterMs":1}}]}}"#
        );
        let error =
            parse_trust_anchors(ANCHOR_PARAM, &document).expect_err("only a 32-byte key is usable");
        assert!(
            matches!(error, TrustError::KeyMaterial { .. }),
            "{material}: {error}"
        );
    }
}

#[test]
fn a_key_identity_that_is_not_the_envelopes_own_is_refused() {
    // The `kid` is 16 raw bytes at a fixed offset in the envelope header, not a
    // free string. A document naming a key the envelope cannot carry would
    // declare an anchor nothing could ever select.
    for key_id in ["", "2026-08-a", "not-a-uuid"] {
        let document = format!(
            r#"{{"schemaVersion":1,"keys":[{{"keyId":"{key_id}","publicKey":"{}","notAfterMs":1}}]}}"#,
            b64(&[1; 32])
        );
        assert!(
            parse_trust_anchors(ANCHOR_PARAM, &document).is_err(),
            "`{key_id}` must not be a usable identity"
        );
    }
}

#[test]
fn a_parameter_past_the_decode_bound_is_refused_before_it_is_parsed() {
    let document = " ".repeat(MAX_PARAMETER_BYTES + 1);
    assert!(matches!(
        parse_trust_anchors(ANCHOR_PARAM, &document),
        Err(TrustError::TooLarge { .. })
    ));
    assert!(matches!(
        parse_cursor_key_ring(CURSOR_PARAM, &document),
        Err(TrustError::TooLarge { .. })
    ));
}

// --- cursor signing ring ----------------------------------------------------------

fn cursor_document(current: (&str, usize), overlap: &[(&str, usize)]) -> String {
    let render = |(id, len): (&str, usize)| {
        format!(r#"{{"keyId":"{id}","material":"{}"}}"#, b64(&vec![9; len]))
    };
    format!(
        r#"{{"schemaVersion":1,"current":{},"overlap":[{}]}}"#,
        render(current),
        overlap
            .iter()
            .copied()
            .map(render)
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[test]
fn a_cursor_ring_signs_under_its_current_key_and_verifies_under_an_overlap_key() {
    let ring = parse_cursor_key_ring(
        CURSOR_PARAM,
        &cursor_document(("2026-08", 32), &[("2026-07", 48)]),
    )
    .expect("a well-formed ring");
    let binding = CursorBinding {
        route: RouteId::SecretsList,
        principal_scope: [1; 32],
        region: Region::EuWest1,
        workspace_id: workspace(),
        session_id: None,
        query_hash: [0; 32],
        order: Order::Ascending,
        snapshot: SnapshotToken::new("secrets").expect("a snapshot token"),
    };
    let tuple = SortTuple::new(vec!["alpha".to_owned()]).expect("a tuple");
    let cursor = encode(ring.current(), &binding, &tuple, moment(1_000)).expect("a cursor");
    assert_eq!(
        decode(&ring, &cursor, &binding, moment(2_000)).expect("the ring verifies its own cursor"),
        tuple
    );

    // The overlap key verifies a cursor it signed, and the ring keeps signing
    // under the current key regardless.
    let retiring = parse_cursor_key_ring(CURSOR_PARAM, &cursor_document(("2026-07", 48), &[]))
        .expect("a ring holding only the retiring key");
    let old = encode(retiring.current(), &binding, &tuple, moment(1_000)).expect("a cursor");
    assert_eq!(
        decode(&ring, &old, &binding, moment(2_000)).expect("an overlap key still verifies"),
        tuple
    );
}

#[test]
fn cursor_material_below_the_security_floor_refuses_the_process() {
    let error = parse_cursor_key_ring(CURSOR_PARAM, &cursor_document(("2026-08", 16), &[]))
        .expect_err("a 16-byte HMAC key is not a key");
    assert!(matches!(error, TrustError::KeyMaterial { .. }), "{error}");
}

#[test]
fn a_cursor_ring_wider_than_the_rotation_overlap_is_refused() {
    let error = parse_cursor_key_ring(
        CURSOR_PARAM,
        &cursor_document(("a", 32), &[("b", 32), ("c", 32), ("d", 32)]),
    )
    .expect_err("three overlap keys is not a rotation");
    assert!(matches!(error, TrustError::CursorRing { .. }), "{error}");
}

#[test]
fn a_cursor_ring_naming_one_identity_twice_is_refused() {
    let error = parse_cursor_key_ring(CURSOR_PARAM, &cursor_document(("a", 32), &[("a", 32)]))
        .expect_err("an ambiguous ring");
    assert!(matches!(error, TrustError::CursorRing { .. }), "{error}");
}

#[test]
fn a_cursor_document_with_an_unknown_member_is_refused() {
    let document = r#"{"schemaVersion":1,"current":{"keyId":"a","material":"AAAA"},"previous":[]}"#;
    let error =
        parse_cursor_key_ring(CURSOR_PARAM, document).expect_err("unknown members are typos");
    assert!(
        matches!(error, TrustError::Malformed { kind, .. } if kind == "cursor signing ring"),
        "{error}"
    );
}

// --- the regional authorization projection ---------------------------------------

#[derive(Debug, Default)]
struct StubProjection {
    placement: Option<Result<WorkspacePlacement, StoreError>>,
    revocation: Option<KeyRevocation>,
    revocation_fails: bool,
}

#[async_trait]
impl AuthorizationProjection for StubProjection {
    async fn read_placement(
        &self,
        workspace: WorkspaceId,
    ) -> Result<WorkspacePlacement, StoreError> {
        match &self.placement {
            Some(Ok(placement)) => Ok(placement.clone()),
            Some(Err(error)) => Err(error.clone()),
            None => Err(StoreError::Misconfigured {
                table: format!("no placement for {workspace}"),
            }),
        }
    }

    async fn read_key_revocation(
        &self,
        _api_key: ApiKeyId,
    ) -> Result<Option<KeyRevocation>, StoreError> {
        if self.revocation_fails {
            return Err(StoreError::Contended);
        }
        Ok(self.revocation.clone())
    }

    async fn read_frontier(&self) -> Result<FeedFrontier, StoreError> {
        Err(StoreError::Contended)
    }
}

fn placement(status: &str) -> WorkspacePlacement {
    WorkspacePlacement {
        workspace: workspace(),
        organization: organization(),
        plane: "dev".to_owned(),
        region: "eu-west-1".to_owned(),
        status: status.to_owned(),
        key_epoch: 10,
        account_epoch: 20,
        revocation_epoch: 30,
        feed_sequence: 77,
        updated_at: moment(1_000),
    }
}

#[tokio::test]
async fn a_key_with_no_revocation_row_projects_the_zero_floor() {
    let projection = RegionalProjection::new(StubProjection::default(), Region::EuWest1);
    let (credential, _) = credential_pair(Region::EuWest1, 5);
    assert_eq!(
        projection.project(&credential).await.expect("it answers"),
        ProjectedEpochs::default()
    );
}

#[tokio::test]
async fn a_published_revocation_raises_the_key_floor_above_every_earlier_assertion() {
    let projection = RegionalProjection::new(
        StubProjection {
            revocation: Some(KeyRevocation {
                api_key: sample::<ApiKeyId>(5),
                revoked_at: moment(1_000),
                revoked_epoch: 11,
            }),
            ..StubProjection::default()
        },
        Region::EuWest1,
    );
    let (credential, _) = credential_pair(Region::EuWest1, 5);
    assert_eq!(
        projection
            .project(&credential)
            .await
            .expect("it answers")
            .key,
        Epoch::new(11)
    );
}

#[tokio::test]
async fn a_credential_for_another_region_is_not_projected_here() {
    let projection = RegionalProjection::new(StubProjection::default(), Region::EuWest1);
    let (credential, _) = credential_pair(Region::UsEast1, 5);
    assert_eq!(
        projection.project(&credential).await,
        Err(ProjectionError::Unknown)
    );
}

#[tokio::test]
async fn an_unreadable_revocation_fails_the_request_closed() {
    let projection = RegionalProjection::new(
        StubProjection {
            revocation_fails: true,
            ..StubProjection::default()
        },
        Region::EuWest1,
    );
    let (credential, _) = credential_pair(Region::EuWest1, 5);
    assert_eq!(
        projection.project(&credential).await,
        Err(ProjectionError::Unavailable)
    );
}

#[tokio::test]
async fn a_placement_projects_its_floors_its_region_and_its_account_policy() {
    for (status, expected) in [
        ("active", AccountState::Active),
        ("paused", AccountState::Paused),
        ("deleting", AccountState::Paused),
    ] {
        let projection = RegionalProjection::new(
            StubProjection {
                placement: Some(Ok(placement(status))),
                ..StubProjection::default()
            },
            Region::EuWest1,
        );
        let state = projection
            .placement(workspace())
            .await
            .expect("a projected placement");
        assert_eq!(state.account_state, expected, "{status}");
        assert_eq!(state.region, Region::EuWest1);
        assert_eq!(
            state.epochs,
            ProjectedEpochs {
                key: Epoch::new(10),
                workspace: Epoch::new(30),
                account: Epoch::new(20),
            }
        );
        // The organization is read from the placement rather than taken from the
        // assertion, because the assertion is what is being checked.
        assert_eq!(state.organization_id, raw(organization()));
    }
}

#[tokio::test]
async fn a_placement_status_outside_the_vocabulary_is_never_guessed_at() {
    let projection = RegionalProjection::new(
        StubProjection {
            placement: Some(Ok(placement("teleported"))),
            ..StubProjection::default()
        },
        Region::EuWest1,
    );
    assert_eq!(
        projection.placement(workspace()).await,
        Err(ProjectionError::Unavailable)
    );
}

#[tokio::test]
async fn an_absent_placement_is_unknown_and_an_unreadable_one_is_unavailable() {
    let absent = RegionalProjection::new(StubProjection::default(), Region::EuWest1);
    assert_eq!(
        absent.placement(workspace()).await,
        Err(ProjectionError::Unknown)
    );

    let unreadable = RegionalProjection::new(
        StubProjection {
            placement: Some(Err(StoreError::Contended)),
            ..StubProjection::default()
        },
        Region::EuWest1,
    );
    assert_eq!(
        unreadable.placement(workspace()).await,
        Err(ProjectionError::Unavailable)
    );
}

// --- the `central-authz` exchange -------------------------------------------------

#[test]
fn a_resolve_request_names_the_key_by_identity_and_digest_and_never_by_value() {
    let (credential, key) = credential_pair(Region::EuWest1, 5);
    let request = resolve_request(
        &credential,
        AssertionAudience::RegionalSession,
        Region::EuWest1,
    )
    .expect("a workspace key resolves");
    assert_eq!(request.key, key);
    assert_eq!(request.region, Region::EuWest1);
    assert_eq!(request.audience, AssertionAudience::RegionalSession);
    // The digest the central plane receives is `SHA-256` over the whole token,
    // which is exactly what the stored verifier is a MAC over.
    assert_eq!(
        request.presented_digest.get(),
        credential.digest().as_bytes()
    );

    let encoded = serde_json::to_string(&request).expect("the request encodes");
    let (token, _) = workspace_key(Region::EuWest1, 5);
    assert!(
        !encoded.contains(&token),
        "the presented token must never appear in the central payload"
    );
    let round_tripped: ResolveWorkspaceKey =
        serde_json::from_str(&encoded).expect("the request round-trips");
    assert_eq!(round_tripped, request);
}

#[test]
fn a_key_minted_for_another_region_never_leaves_this_one() {
    let (credential, _) = credential_pair(Region::UsEast1, 5);
    assert_eq!(
        resolve_request(
            &credential,
            AssertionAudience::RegionalSession,
            Region::EuWest1
        ),
        Err(AuthFailure::MalformedCredential)
    );
}

#[test]
fn a_credential_that_is_not_a_workspace_key_is_refused_before_anything_reads_it() {
    // A regional host accepts one credential. Refusing at construction means the
    // three places that need the key id cannot each re-parse and disagree.
    for value in [
        b"aex_at_not_a_workspace_key".to_vec(),
        b"Bearer something".to_vec(),
        Vec::new(),
        vec![b'a'; 8_192],
        b"aex_wk_euw1_short_secret".to_vec(),
    ] {
        assert_eq!(
            PresentedCredential::new(value.clone()).err(),
            Some(AuthFailure::MalformedCredential),
            "{}",
            String::from_utf8_lossy(&value)
        );
    }
}

#[test]
fn a_credential_never_renders_its_plaintext_or_its_digest() {
    let (credential, key) = credential_pair(Region::EuWest1, 5);
    let rendered = format!("{credential:?}");
    let (token, _) = workspace_key(Region::EuWest1, 5);
    assert!(!rendered.contains(&token), "{rendered}");
    assert!(
        !rendered.contains(&b64(credential.digest().as_bytes())),
        "{rendered}"
    );
    assert!(rendered.contains(&key.to_string()), "{rendered}");
}

#[test]
fn a_refusal_is_carried_through_as_a_decision_and_not_as_an_outage() {
    // The two answers a caller must render differently. Collapsing either into
    // `SourceUnavailable` would make a revoked key look like a central outage
    // and be retried forever.
    assert_eq!(
        issued_assertion(AssertionResponse::Refused {
            reason: AssertionRefusal::NotAuthorized
        }),
        Err(AuthFailure::Refused)
    );
    assert_eq!(
        issued_assertion(AssertionResponse::Refused {
            reason: AssertionRefusal::AccountStateUnavailable
        }),
        Err(AuthFailure::AccountStateUnavailable)
    );
}

#[test]
fn an_issued_answer_decodes_into_the_exact_envelope_that_was_signed() {
    let (credential, _) = credential_pair(Region::EuWest1, 5);
    let claims = claims(
        &credential,
        AssertionAudience::RegionalSecret,
        aex_control_domain::ScopeSet::EMPTY,
        u64::try_from(NOW_MS).expect("positive"),
    );
    let response = answer(&claims);
    let encoded = serde_json::to_vec(&response).expect("the answer encodes");
    assert_eq!(
        issued_assertion(decode_response(&encoded).expect("it decodes")).expect("an envelope"),
        envelope(&claims)
    );
}

#[test]
fn a_response_past_the_decode_bound_is_refused_before_it_is_parsed() {
    let payload = vec![b' '; 64 * 1_024 + 1];
    assert_eq!(
        decode_response(&payload),
        Err(AuthFailure::MalformedAssertion)
    );
}

// --- the composed edge ------------------------------------------------------------

struct StubSource {
    assertion: Assertion,
    calls: Mutex<usize>,
}

#[async_trait]
impl AssertionSource for StubSource {
    async fn obtain(&self, _credential: &PresentedCredential) -> Result<Assertion, AuthFailure> {
        *self.calls.lock().expect("an uncontended fixture") += 1;
        Ok(self.assertion)
    }
}

struct StubProjectionReader {
    credential_floor: ProjectedEpochs,
    placement: Result<ProjectedState, ProjectionError>,
}

#[async_trait]
impl ProjectionReader for StubProjectionReader {
    async fn project(
        &self,
        _credential: &PresentedCredential,
    ) -> Result<ProjectedEpochs, ProjectionError> {
        Ok(self.credential_floor)
    }

    async fn placement(&self, _workspace: WorkspaceId) -> Result<ProjectedState, ProjectionError> {
        self.placement
    }
}

struct FixedClock(i64);

impl EdgeClock for FixedClock {
    fn now(&self) -> Timestamp {
        moment(self.0)
    }
}

fn projected(account_state: AccountState, region: Region) -> ProjectedState {
    ProjectedState {
        epochs: ProjectedEpochs {
            key: Epoch::new(10),
            workspace: Epoch::new(30),
            account: Epoch::new(20),
        },
        organization_id: raw(organization()),
        account_state,
        region,
    }
}

/// A route this deployable can admit without a replay identity, so the edge test
/// exercises admission rather than idempotency. Drawn from the generated table so
/// a re-authored route cannot silently make the fixture meaningless.
fn plain_route() -> RouteId {
    *RouteId::ALL
        .iter()
        .find(|id| {
            let descriptor = route(**id);
            descriptor.plane == Plane::Regional
                && descriptor.method == HttpMethod::Get
                && descriptor.idempotency == IdempotencyKind::None
                && descriptor.required_scope.is_some()
                && !descriptor.pause_exempt
        })
        .expect("the regional table declares a scoped, non-exempt read")
}

type Edge = RegionalEdge<StubSource, StubProjectionReader, FixedClock>;

fn binding() -> EdgeBinding {
    EdgeBinding {
        plane: AssertionPlane::Dev,
        audience: AssertionAudience::RegionalSession,
        region: Region::EuWest1,
        cache_budget_bytes: 64 * 1_024,
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
    }
}

fn edge_over(
    assertion: &Assertion,
    floor: ProjectedEpochs,
    placement: Result<ProjectedState, ProjectionError>,
) -> Edge {
    RegionalEdge::new(
        StubSource {
            assertion: *assertion,
            calls: Mutex::new(0),
        },
        keys(),
        StubProjectionReader {
            credential_floor: floor,
            placement,
        },
        FixedClock(NOW_MS + 2_000),
        binding(),
    )
    .expect("the cache budget holds an entry")
}

fn edge(
    credential: &PresentedCredential,
    scopes: aex_control_domain::ScopeSet,
    floor: ProjectedEpochs,
    placement: Result<ProjectedState, ProjectionError>,
) -> Edge {
    let claims = claims(
        credential,
        AssertionAudience::RegionalSession,
        scopes,
        u64::try_from(NOW_MS).expect("positive"),
    );
    edge_over(&envelope(&claims), floor, placement)
}

fn headers(credential: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {credential}")).expect("a header value"),
    );
    headers
}

async fn admit(
    edge: &Edge,
    route: RouteId,
    headers: &HeaderMap,
) -> Result<aex_regional_http::context::RequestContext, ErrorCode> {
    let request_id = RequestId::parse("edge-fixture").expect("a request id");
    edge.admit(&AdmissionRequest {
        request_id: &request_id,
        route,
        method: aex_wire::routes::route(route).method,
        headers,
        body: &[],
    })
    .await
    .map_err(|failure| failure.code)
}

fn scoped(id: RouteId) -> aex_control_domain::ScopeSet {
    route(id)
        .required_scope
        .into_iter()
        .collect::<aex_control_domain::ScopeSet>()
}

#[tokio::test]
async fn a_verified_credential_is_admitted_with_the_projected_account_policy() {
    let (token, key) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    let context = admit(&edge, id, &headers(&token))
        .await
        .expect("a current credential is admitted");
    assert_eq!(context.auth.workspace_id, workspace());
    assert_eq!(context.auth.organization_id, organization());
    assert_eq!(context.auth.account_state, AccountState::Active);
    assert_eq!(context.auth.placement, Region::EuWest1);
    assert_eq!(context.route, id);
    assert_eq!(
        context.auth.principal,
        aex_wire::idempotency::PrincipalScope::WorkspaceKey {
            key,
            workspace: workspace(),
            organization: organization(),
        }
    );
    // The named epoch record is a projection of the envelope's subject slots,
    // not a flat triple a handler has to interpret.
    assert_eq!(context.auth.epochs.key, 10);
    assert_eq!(context.auth.epochs.workspace, 30);
    assert_eq!(context.auth.epochs.account, 20);
    assert_eq!(context.auth.epochs.membership, 0);
    assert_eq!(
        context.auth.credential_binding,
        credential.expected_binding()
    );
}

#[tokio::test]
async fn an_assertion_minted_for_another_credential_is_refused() {
    // The binding is inside the signed body at a fixed offset, so an envelope for
    // another key fails verification rather than needing a sibling field checked
    // first — and it can never enter the cache, which stores only verified
    // authorizations.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let (other, _) = credential_pair(Region::EuWest1, 6);
    let id = plain_route();
    let foreign = claims(
        &other,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS).expect("positive"),
    );
    let edge = edge_over(
        &envelope(&foreign),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn an_assertion_minted_for_another_edge_is_refused() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    for other in [
        AssertionAudience::RegionalSecret,
        AssertionAudience::RegionalObservation,
        AssertionAudience::RegionalOtlp,
        AssertionAudience::RegionalStream,
    ] {
        let foreign = claims(
            &credential,
            other,
            scoped(id),
            u64::try_from(NOW_MS).expect("positive"),
        );
        let edge = edge_over(
            &envelope(&foreign),
            ProjectedEpochs::default(),
            Ok(projected(AccountState::Active, Region::EuWest1)),
        );
        assert_eq!(
            admit(&edge, id, &headers(&token)).await.err(),
            Some(ErrorCode::Unauthenticated),
            "{other:?}"
        );
    }
}

#[tokio::test]
async fn an_expired_assertion_is_refused_even_though_it_verifies() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    // The clock is `NOW_MS + 2_000`; this envelope expired at `NOW_MS + 1_000`.
    let lapsed = claims(
        &credential,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS - 29_000).expect("positive"),
    );
    let edge = edge_over(
        &envelope(&lapsed),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn an_assertion_for_a_person_is_refused_by_a_regional_edge() {
    // A person's envelope carries `user` and `membership` subjects the regional
    // projection holds nothing for, so admitting one would admit a credential
    // whose revocation cannot be observed here.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let mut person = claims(
        &credential,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS).expect("positive"),
    );
    person.principal_kind = PrincipalKind::UserSession;
    let edge = edge_over(
        &envelope(&person),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn an_assertion_carrying_a_subject_this_region_cannot_project_is_refused() {
    // The fail-closed direction: an unprojectable subject is stale, never
    // "no reason to refuse was found".
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let mut smuggled = claims(
        &credential,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS).expect("positive"),
    );
    smuggled.epochs = EpochSlots::new(&[
        EpochSlot {
            kind: EpochSubjectKind::Key,
            id: credential.key_id_raw(),
            epoch: Epoch::new(10),
        },
        EpochSlot {
            kind: EpochSubjectKind::Membership,
            id: Uuid::from_u128(0x99),
            epoch: Epoch::new(1),
        },
    ])
    .expect("two distinct subjects");
    let edge = edge_over(
        &envelope(&smuggled),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn a_revocation_published_after_the_assertion_was_minted_still_wins() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    // The assertion claims key epoch 10; the credential's own floor is now 11.
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs {
            key: Epoch::new(11),
            ..ProjectedEpochs::default()
        },
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn a_placement_whose_floors_moved_refuses_a_valid_assertion() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    for mutate in [
        |state: &mut ProjectedState| state.epochs.account = Epoch::new(21),
        |state: &mut ProjectedState| state.epochs.workspace = Epoch::new(31),
        |state: &mut ProjectedState| state.epochs.key = Epoch::new(11),
    ] {
        let mut state = projected(AccountState::Active, Region::EuWest1);
        mutate(&mut state);
        let edge = edge(
            &credential,
            scoped(id),
            ProjectedEpochs::default(),
            Ok(state),
        );
        assert_eq!(
            admit(&edge, id, &headers(&token)).await.err(),
            Some(ErrorCode::Unauthenticated)
        );
    }
}

#[tokio::test]
async fn an_assertion_naming_another_organization_than_the_placement_is_refused() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.organization_id = raw(sample::<OrganizationId>(9));
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Ok(state),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn a_paused_account_loses_every_route_the_table_does_not_exempt() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Paused, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::AccountPaused)
    );
}

#[tokio::test]
async fn an_assertion_minted_under_a_pause_pauses_even_when_the_placement_says_active() {
    // The two states differ only when one of them is stale, and the safe reading
    // of a stale account state is the restrictive one.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let mut paused = claims(
        &credential,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS).expect("positive"),
    );
    paused.account_state = AssertedAccountState::PausedTopUpRequired;
    let edge = edge_over(
        &envelope(&paused),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::AccountPaused)
    );
}

#[tokio::test]
async fn a_workspace_placed_in_another_region_is_refused_here() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::UsEast1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::WrongWorkspaceRegion)
    );
}

#[tokio::test]
async fn an_unavailable_projection_is_a_refusal_and_never_an_optimistic_admission() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Err(ProjectionError::Unavailable),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::AccountStateUnavailable)
    );
}

#[tokio::test]
async fn a_credential_carrying_none_of_the_declared_scope_is_refused() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        aex_control_domain::ScopeSet::EMPTY,
        ProjectedEpochs::default(),
        Ok(projected(AccountState::Active, Region::EuWest1)),
    );
    assert_eq!(
        admit(&edge, id, &headers(&token)).await.err(),
        Some(ErrorCode::InsufficientScope)
    );
}

#[tokio::test]
async fn a_request_with_no_credential_never_reaches_the_projection() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(
        &credential,
        scoped(id),
        ProjectedEpochs::default(),
        Err(ProjectionError::Unavailable),
    );
    assert_eq!(
        admit(&edge, id, &HeaderMap::new()).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

/// A projection reader that counts how often the placement is consulted.
struct CountingProjection {
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    state: ProjectedState,
}

#[async_trait]
impl ProjectionReader for CountingProjection {
    async fn project(
        &self,
        _credential: &PresentedCredential,
    ) -> Result<ProjectedEpochs, ProjectionError> {
        Ok(ProjectedEpochs::default())
    }

    async fn placement(&self, _workspace: WorkspaceId) -> Result<ProjectedState, ProjectionError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.state)
    }
}

#[tokio::test]
async fn the_placement_is_read_on_every_request_even_when_the_assertion_is_cached() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let claims = claims(
        &credential,
        AssertionAudience::RegionalSession,
        scoped(id),
        u64::try_from(NOW_MS).expect("positive"),
    );
    let source = StubSource {
        assertion: envelope(&claims),
        calls: Mutex::new(0),
    };
    let edge = RegionalEdge::new(
        source,
        keys(),
        CountingProjection {
            reads: std::sync::Arc::clone(&reads),
            state: projected(AccountState::Active, Region::EuWest1),
        },
        FixedClock(NOW_MS + 2_000),
        binding(),
    )
    .expect("the cache budget holds an entry");

    let request_id = RequestId::parse("edge-fixture").expect("a request id");
    let headers = headers(&token);
    for _ in 0..3_u8 {
        edge.admit(&AdmissionRequest {
            request_id: &request_id,
            route: id,
            method: route(id).method,
            headers: &headers,
            body: &[],
        })
        .await
        .expect("each request is admitted");
    }
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "a cached assertion must not cache the placement with it"
    );
}

#[test]
fn the_fixture_route_set_is_drawn_from_the_generated_table() {
    // Guards the two fixtures above: if the table ever stops declaring a scoped,
    // non-exempt regional read, the edge tests would silently test nothing.
    let id = plain_route();
    let descriptor = route(id);
    assert_eq!(descriptor.plane, Plane::Regional);
    assert!(descriptor.required_scope.is_some());
    assert!(!descriptor.pause_exempt);
    assert_eq!(descriptor.idempotency, IdempotencyKind::None);
}
