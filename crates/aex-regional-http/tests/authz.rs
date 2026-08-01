//! Black-box requirements for the concrete edge adapters: the trust anchors, the
//! cursor signing ring, the `central-authz` exchange payloads, the regional
//! authorization projection, and the whole admission chain composed over them.
//!
//! Nothing here reaches AWS. Every adapter is split so the decision is a pure
//! function of bytes and the client call is the only thing left un-exercised,
//! which is exactly the part a live companion owns.

use std::sync::Mutex;

use aex_internal_contracts::assertion::{
    AssertionAudience, AssertionSignature, AuthorizationAssertion, CredentialDigest,
    ResolveWorkspaceKey, SignedAssertionEnvelope, credential_bound_signing_input,
};
use aex_internal_contracts::{Epoch, SchemaVersion};
use aex_regional_http::assertion::{
    AssertionSource, AuthFailure, KeyVerifier, PresentedCredential, ProjectedEpochs,
    SignedAssertion,
};
use aex_regional_http::authz::{
    Ed25519Anchors, MAX_PARAMETER_BYTES, MAX_TRUST_ANCHORS, RegionalProjection, TrustError,
    decode_response, parse_cursor_key_ring, parse_trust_anchors, resolve_request, signed_assertion,
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
use aex_wire::idempotency::{PrincipalKind, PrincipalScope};
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId, UserId, Uuid7, WorkspaceId};
use aex_wire::routes::{Plane, RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{HttpMethod, Region, RequestId, Timestamp};
use async_trait::async_trait;
use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use http::{HeaderMap, HeaderValue};

// --- fixtures -------------------------------------------------------------------

const ANCHOR_PARAM: &str = "/aex/dev/authz/verify-keys";
const CURSOR_PARAM: &str = "/aex/dev/regional/cursor-signing-key";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sample<I: PrefixedId>(seed: u8) -> I {
    I::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]))
}

fn workspace() -> WorkspaceId {
    sample::<WorkspaceId>(2)
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

fn credential(region: Region, seed: u8) -> (PresentedCredential, ApiKeyId) {
    let (token, key) = workspace_key(region, seed);
    (
        PresentedCredential::new(token.into_bytes()).expect("a workspace key is a credential"),
        key,
    )
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[13; 32])
}

fn anchor_document(key_id: &str, public_key: &[u8]) -> String {
    format!(
        r#"{{"schemaVersion":1,"keys":[{{"keyId":"{key_id}","publicKey":"{}"}}]}}"#,
        b64(public_key)
    )
}

fn anchors() -> Ed25519Anchors {
    parse_trust_anchors(
        ANCHOR_PARAM,
        &anchor_document("2026-08-a", &signing_key().verifying_key().to_bytes()),
    )
    .expect("a well-formed anchor document")
}

fn assertion(audience: AssertionAudience, scopes: ScopeSet, now: i64) -> AuthorizationAssertion {
    AuthorizationAssertion {
        schema_version: SchemaVersion::V1,
        principal: PrincipalScope::Account {
            user: sample::<UserId>(3),
            organization: Some(sample::<OrganizationId>(4)),
        },
        principal_kind: PrincipalKind::Account,
        workspace: Some(workspace()),
        organization: sample::<OrganizationId>(4),
        region: Region::EuWest1,
        scopes,
        key_epoch: Epoch(10),
        account_epoch: Epoch(20),
        revocation_epoch: Epoch(30),
        issued_at: moment(now),
        expires_at: moment(now + 30_000),
        audience,
    }
}

/// Signs an assertion the way `central-authz` must: over the one published
/// credential-bound signing input, and never over anything else.
fn envelope(
    assertion: AuthorizationAssertion,
    binding: CredentialDigest,
    key_id: &str,
) -> SignedAssertionEnvelope {
    let signature = signing_key()
        .sign(&credential_bound_signing_input(&assertion, &binding))
        .to_bytes()
        .to_vec();
    SignedAssertionEnvelope {
        schema_version: SchemaVersion::V1,
        assertion,
        key_id: key_id.to_owned(),
        credential_binding: binding,
        signature: AssertionSignature::new(signature).expect("64 bytes is in bounds"),
    }
}

// --- trust anchors ---------------------------------------------------------------

#[test]
fn a_well_formed_anchor_document_yields_exactly_its_declared_keys() {
    let anchors = anchors();
    assert_eq!(anchors.len(), 1);
    assert!(!anchors.is_empty());
    assert_eq!(anchors.key_ids(), vec!["2026-08-a"]);
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
                r#"{{"keyId":"k{index}","publicKey":"{}"}}"#,
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
        r#"{{"schemaVersion":1,"keys":[{{"keyId":"a","publicKey":"{0}"}},{{"keyId":"a","publicKey":"{0}"}}]}}"#,
        b64(&[1; 32])
    );
    let error = parse_trust_anchors(ANCHOR_PARAM, &document).expect_err("an ambiguous ring");
    assert_eq!(
        error,
        TrustError::DuplicateKeyId {
            name: ANCHOR_PARAM.to_owned(),
            key_id: "a".to_owned()
        }
    );
}

#[test]
fn key_material_that_is_not_exactly_an_ed25519_public_key_is_refused() {
    for material in [b64(&[1; 31]), b64(&[1; 33]), "not base64".to_owned()] {
        let document =
            format!(r#"{{"schemaVersion":1,"keys":[{{"keyId":"a","publicKey":"{material}"}}]}}"#);
        let error =
            parse_trust_anchors(ANCHOR_PARAM, &document).expect_err("only a 32-byte key is usable");
        assert!(
            matches!(error, TrustError::KeyMaterial { .. }),
            "{material}: {error}"
        );
    }
}

#[test]
fn an_unusable_key_identity_is_refused() {
    for key_id in ["", "has space", "control\\u0007"] {
        let document = format!(
            r#"{{"schemaVersion":1,"keys":[{{"keyId":"{key_id}","publicKey":"{}"}}]}}"#,
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

#[test]
fn a_real_signature_verifies_under_its_own_identity_and_under_no_other() {
    let anchors = anchors();
    let message = b"aex:credential-bound-authorization-assertion:v1\x1f{}";
    let signature = signing_key().sign(message).to_bytes();
    assert!(anchors.verify("2026-08-a", message, &signature));
    assert!(
        !anchors.verify("2026-08-b", message, &signature),
        "an identity outside the ring must never verify"
    );
}

#[test]
fn a_tampered_message_or_a_misshapen_signature_never_verifies() {
    let anchors = anchors();
    let message = b"the covered bytes";
    let signature = signing_key().sign(message).to_bytes();
    assert!(!anchors.verify("2026-08-a", b"other bytes", &signature));
    assert!(!anchors.verify("2026-08-a", message, &signature[..63]));
    assert!(!anchors.verify("2026-08-a", message, &[0; 64]));
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

// --- the `central-authz` exchange -------------------------------------------------

#[test]
fn a_resolve_request_names_the_key_by_identity_and_digest_and_never_by_value() {
    let (credential, key) = credential(Region::EuWest1, 5);
    let request = resolve_request(
        &credential,
        AssertionAudience::RegionalSession,
        Region::EuWest1,
    )
    .expect("a workspace key resolves");
    assert_eq!(request.key, key);
    assert_eq!(request.region, Region::EuWest1);
    assert_eq!(request.audience, AssertionAudience::RegionalSession);
    // The digest the central plane receives is exactly the binding this edge
    // derived, which is what makes the answer's binding comparable.
    assert_eq!(*request.presented_digest.get(), credential.binding());

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
    let (credential, _) = credential(Region::UsEast1, 5);
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
fn a_credential_that_is_not_a_workspace_key_is_refused_before_any_invoke() {
    let credential = PresentedCredential::new(b"aex_at_not_a_workspace_key".to_vec())
        .expect("bounded bytes are a credential");
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
fn an_assertion_minted_for_another_credential_never_reaches_the_cache() {
    let (credential, _) = credential(Region::EuWest1, 5);
    let (other, _) = credential_for(6);
    let answer = envelope(
        assertion(AssertionAudience::RegionalSession, ScopeSet::empty(), 1_000),
        CredentialDigest::new(other.binding()),
        "2026-08-a",
    );
    assert_eq!(
        signed_assertion(answer, &credential).expect_err("a foreign binding"),
        AuthFailure::CredentialBinding
    );
}

fn credential_for(seed: u8) -> (PresentedCredential, ApiKeyId) {
    credential(Region::EuWest1, seed)
}

#[test]
fn a_matching_answer_becomes_a_transport_whose_signature_verifies() {
    let (credential, _) = credential(Region::EuWest1, 5);
    let answer = envelope(
        assertion(AssertionAudience::RegionalSession, ScopeSet::empty(), 1_000),
        CredentialDigest::new(credential.binding()),
        "2026-08-a",
    );
    let transport = signed_assertion(answer, &credential).expect("a bound answer");
    assert!(
        anchors().verify(
            "2026-08-a",
            &transport.signing_input(),
            transport.signature()
        ),
        "the issuer and the edge must agree on the covered bytes"
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

#[test]
fn a_signed_envelope_round_trips_through_the_internal_wire() {
    let (credential, _) = credential(Region::EuWest1, 5);
    let answer = envelope(
        assertion(AssertionAudience::RegionalSecret, ScopeSet::empty(), 1_000),
        CredentialDigest::new(credential.binding()),
        "2026-08-a",
    );
    let encoded = serde_json::to_vec(&answer).expect("the envelope encodes");
    assert_eq!(decode_response(&encoded).expect("it decodes"), answer);
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
        organization: sample::<OrganizationId>(4),
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
    let (credential, _) = credential(Region::EuWest1, 5);
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
    let (credential, _) = credential(Region::EuWest1, 5);
    assert_eq!(
        projection
            .project(&credential)
            .await
            .expect("it answers")
            .key,
        11
    );
}

#[tokio::test]
async fn a_credential_for_another_region_is_not_projected_here() {
    let projection = RegionalProjection::new(StubProjection::default(), Region::EuWest1);
    let (credential, _) = credential(Region::UsEast1, 5);
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
    let (credential, _) = credential(Region::EuWest1, 5);
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
                key: 10,
                account: 20,
                revocation: 30
            }
        );
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

// --- the composed edge ------------------------------------------------------------

struct StubSource {
    envelope: SignedAssertionEnvelope,
    calls: Mutex<usize>,
}

#[async_trait]
impl AssertionSource for StubSource {
    async fn obtain(
        &self,
        credential: &PresentedCredential,
    ) -> Result<SignedAssertion, AuthFailure> {
        *self.calls.lock().expect("an uncontended fixture") += 1;
        signed_assertion(self.envelope.clone(), credential)
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
            key: 10,
            account: 20,
            revocation: 30,
        },
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

fn edge(
    credential: &PresentedCredential,
    scopes: ScopeSet,
    floor: ProjectedEpochs,
    placement: Result<ProjectedState, ProjectionError>,
) -> RegionalEdge<StubSource, Ed25519Anchors, StubProjectionReader, FixedClock> {
    let envelope = envelope(
        assertion(AssertionAudience::RegionalSession, scopes, 1_000),
        CredentialDigest::new(credential.binding()),
        "2026-08-a",
    );
    RegionalEdge::new(
        StubSource {
            envelope,
            calls: Mutex::new(0),
        },
        anchors(),
        StubProjectionReader {
            credential_floor: floor,
            placement,
        },
        FixedClock(2_000),
        EdgeBinding {
            audience: AssertionAudience::RegionalSession,
            region: Region::EuWest1,
            cache_budget_bytes: 64 * 1_024,
            limits: EffectiveLimits {
                json_body_bytes: 65_536,
                query_page_items: 100,
                query_page_bytes: 1_048_576,
            },
        },
    )
    .expect("the cache budget holds an entry")
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
    edge: &RegionalEdge<StubSource, Ed25519Anchors, StubProjectionReader, FixedClock>,
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

fn scoped(route: RouteId) -> ScopeSet {
    ScopeSet::new(route_scope(route))
}

fn route_scope(id: RouteId) -> Vec<aex_wire::scopes::ScopeId> {
    route(id).required_scope.into_iter().collect()
}

#[tokio::test]
async fn a_verified_credential_is_admitted_with_the_projected_account_policy() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
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
    assert_eq!(context.auth.account_state, AccountState::Active);
    assert_eq!(context.auth.placement, Region::EuWest1);
    assert_eq!(context.route, id);
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
            key: 11,
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
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.epochs.account = 21;
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
        ScopeSet::empty(),
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

    let source = StubSource {
        envelope: envelope(
            assertion(AssertionAudience::RegionalSession, scoped(id), 1_000),
            CredentialDigest::new(credential.binding()),
            "2026-08-a",
        ),
        calls: Mutex::new(0),
    };
    let edge = RegionalEdge::new(
        source,
        anchors(),
        CountingProjection {
            reads: std::sync::Arc::clone(&reads),
            state: projected(AccountState::Active, Region::EuWest1),
        },
        FixedClock(2_000),
        EdgeBinding {
            audience: AssertionAudience::RegionalSession,
            region: Region::EuWest1,
            cache_budget_bytes: 64 * 1_024,
            limits: EffectiveLimits {
                json_body_bytes: 65_536,
                query_page_items: 100,
                query_page_bytes: 1_048_576,
            },
        },
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
