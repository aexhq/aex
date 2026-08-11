//! Black-box requirements for the concrete edge adapters: the cursor signing
//! ring, the regional authorization projection, and the whole admission chain
//! composed over them.
//!
//! Nothing here reaches AWS, and after this revision nothing here *could*: the
//! credential check is a pure function of the presented token, the pepper ring
//! resolved at cold start and the row the projection returned. Every adapter is
//! split so the decision is a pure function of bytes and the client call is the
//! only thing left un-exercised, which is exactly the part a live companion
//! owns.

use aex_control_domain::epoch::Epoch;
use aex_identity_domain::credential::{Pepper, PresentedDigest, verifier};
use aex_internal_contracts::assertion::{AssertionAudience, AudienceSet};
use aex_regional_http::authz::{
    MAX_DOCUMENT_BYTES, RegionalProjection, TrustError, parse_cursor_key_ring, parse_pepper_ring,
};
use aex_regional_http::context::{AccountState, EffectiveLimits};
use aex_regional_http::credential::{
    AuthFailure, PepperRing, PresentedCredential, ProjectedEpochs, StoredVerifier,
};
use aex_regional_http::cursor::{CursorBinding, Order, SnapshotToken, SortTuple, decode, encode};
use aex_regional_http::edge::{
    EdgeBinding, EdgeClock, ProjectedState, ProjectionError, ProjectionReader, RegionalEdge,
};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::AuthorizationProjection;
use aex_session_dynamodb::wire_pending::{
    AdmissionSnapshot, EdgeLimits, FeedFrontier, KeyAuthorization, KeyAuthorizationState,
    WorkspacePlacement,
};
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IdempotencyKind;
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::routes::{Plane, RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{HttpMethod, Region, RequestId, Timestamp};
use async_trait::async_trait;
use base64::Engine as _;
use http::{HeaderMap, HeaderValue};
use std::sync::Mutex;
use uuid::Uuid;

// --- fixtures -------------------------------------------------------------------

const PEPPER_SECRET: &str = "aex/dev/central/token-pepper";
const CURSOR_PARAM: &str = "/aex/dev/regional/cursor-signing-key";
const NOW_MS: i64 = 1_754_051_698_000;

/// The pepper every fixture verifier is computed under.
const PEPPER: [u8; 32] = [11; 32];

/// The audience the edge under test accepts.
const AUDIENCE: AssertionAudience = AssertionAudience::RegionalSession;

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

/// The Crockford suffix a workspace-key segment spells an id as.
fn key_suffix<I: PrefixedId>(id: I) -> String {
    String::from_utf8(id.uuid7().encode_suffix().to_vec()).expect("Crockford is ASCII")
}

/// A syntactically complete workspace API key, and the identity it embeds.
fn workspace_key(region: Region, seed: u8) -> (String, ApiKeyId) {
    let uuid = Uuid7::compose(1_754_051_696_789, [seed; 10]);
    let key = ApiKeyId::from_uuid7(uuid);
    let token = format!(
        "aex_wk_{}_{}_{}_{}",
        region.code(),
        key_suffix(workspace()),
        key_suffix(key),
        b64(&[seed; 32])
    );
    (token, key)
}

fn credential_pair(region: Region, seed: u8) -> (PresentedCredential, ApiKeyId) {
    let (token, key) = workspace_key(region, seed);
    (
        PresentedCredential::new(token.into_bytes()).expect("a workspace key is a credential"),
        key,
    )
}

fn pepper_document(entries: &[(u16, [u8; 32])]) -> String {
    let peppers: Vec<String> = entries
        .iter()
        .map(|(version, material)| {
            format!(r#"{{"version":{version},"material":"{}"}}"#, b64(material))
        })
        .collect();
    format!(r#"{{"schemaVersion":1,"peppers":[{}]}}"#, peppers.join(","))
}

fn ring() -> PepperRing {
    parse_pepper_ring(PEPPER_SECRET, &pepper_document(&[(1, PEPPER)])).expect("a usable ring")
}

/// The verifier the control plane stores for a token, under `pepper`.
fn stored_for(token: &str, version: u16, pepper: [u8; 32]) -> StoredVerifier {
    let keyed = verifier(&Pepper::new(pepper), &PresentedDigest::of(token));
    StoredVerifier::new(*keyed.as_bytes(), version)
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
        route: RouteId::SessionsList,
        principal_scope: [1; 32],
        region: Region::EuWest1,
        workspace_id: workspace(),
        session_id: None,
        query_hash: [0; 32],
        order: Order::Ascending,
        snapshot: SnapshotToken::new("sessions").expect("a snapshot token"),
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

#[test]
fn a_parameter_past_the_decode_bound_is_refused_before_it_is_parsed() {
    let document = " ".repeat(MAX_DOCUMENT_BYTES + 1);
    assert!(matches!(
        parse_pepper_ring(PEPPER_SECRET, &document),
        Err(TrustError::TooLarge { .. })
    ));
    assert!(matches!(
        parse_cursor_key_ring(CURSOR_PARAM, &document),
        Err(TrustError::TooLarge { .. })
    ));
}

// --- the regional authorization projection ---------------------------------------

#[derive(Debug)]
struct StubProjection {
    snapshot: Result<AdmissionSnapshot, StoreError>,
}

impl Default for StubProjection {
    fn default() -> Self {
        Self {
            snapshot: Ok(snapshot("active", KeyAuthorizationState::Active)),
        }
    }
}

#[async_trait]
impl AuthorizationProjection for StubProjection {
    async fn read_placement(
        &self,
        _workspace: WorkspaceId,
    ) -> Result<WorkspacePlacement, StoreError> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.placement.clone())
            .map_err(Clone::clone)
    }

    async fn read_key_authorization(
        &self,
        _api_key: ApiKeyId,
    ) -> Result<KeyAuthorization, StoreError> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.key.clone())
            .map_err(Clone::clone)
    }

    async fn read_admission_snapshot(
        &self,
        _api_key: ApiKeyId,
        _workspace: WorkspaceId,
    ) -> Result<AdmissionSnapshot, StoreError> {
        self.snapshot.clone()
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

fn snapshot(status: &str, state: KeyAuthorizationState) -> AdmissionSnapshot {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    AdmissionSnapshot {
        key: KeyAuthorization {
            api_key: sample::<ApiKeyId>(5),
            workspace: workspace(),
            organization: organization(),
            region: "eu-west-1".to_owned(),
            state,
            verifier: *verifier(&Pepper::new(PEPPER), &PresentedDigest::of(&token)).as_bytes(),
            pepper_version: 1,
            audiences: AudienceSet::EMPTY.insert(AUDIENCE),
            scopes: ScopeSet::new(route(plain_route()).required_scope),
            key_epoch: 10,
            projection_sequence: 3,
            updated_at: moment(1_000),
        },
        placement: placement(status),
        limits: EdgeLimits {
            workspace: workspace(),
            revision: 4,
            json_body_bytes: 1_048_576,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 1_000,
            query_page_bytes: 8 * 1_024 * 1_024,
            changed_at: moment(1_000),
        },
    }
}

#[tokio::test]
async fn a_snapshot_projects_everything_admission_needs_and_nothing_it_does_not() {
    for (status, expected) in [
        ("active", AccountState::Active),
        ("paused", AccountState::Paused),
        ("deleting", AccountState::Paused),
    ] {
        let projection = RegionalProjection::new(
            StubProjection {
                snapshot: Ok(snapshot(status, KeyAuthorizationState::Active)),
            },
            Region::EuWest1,
        );
        let state = projection
            .snapshot(sample::<ApiKeyId>(5), workspace())
            .await
            .expect("a projected snapshot");
        assert_eq!(state.account_state, expected, "{status}");
        assert_eq!(state.region, Region::EuWest1);
        assert!(!state.key_revoked);
        assert_eq!(state.workspace_id, workspace());
        assert_eq!(
            state.epochs,
            ProjectedEpochs {
                key: Epoch::new(10),
                workspace: Epoch::new(30),
                account: Epoch::new(20),
            }
        );
        assert_eq!(state.organization_id, raw(organization()));
        assert_eq!(state.limits.json_body_bytes, 1_048_576);
        assert_eq!(state.limits.query_page_items, 1_000);
        // The four facts that used to arrive in a signed envelope from another
        // plane now arrive on the row this read already returned.
        assert_eq!(state.verifier.pepper_version(), 1);
        assert!(state.audiences.contains(AUDIENCE));
        assert_eq!(
            state.scopes,
            ScopeSet::new(route(plain_route()).required_scope)
        );
        let (token, _) = workspace_key(Region::EuWest1, 5);
        assert_eq!(state.verifier, stored_for(&token, 1, PEPPER));
    }
}

#[tokio::test]
async fn a_revoked_key_row_raises_the_floor_and_reports_the_revocation() {
    let mut revoked = snapshot("active", KeyAuthorizationState::Revoked);
    revoked.key.key_epoch = 11;
    let projection = RegionalProjection::new(
        StubProjection {
            snapshot: Ok(revoked),
        },
        Region::EuWest1,
    );
    let state = projection
        .snapshot(sample::<ApiKeyId>(5), workspace())
        .await
        .expect("it answers");
    assert!(state.key_revoked);
    // The key row's own epoch wins over the placement's when it is higher: it is
    // the one a revocation raises.
    assert_eq!(state.epochs.key, Epoch::new(11));
}

#[tokio::test]
async fn a_snapshot_placed_in_another_region_is_not_projected_here() {
    let mut foreign = snapshot("active", KeyAuthorizationState::Active);
    foreign.key.region = "us-east-1".to_owned();
    foreign.placement.region = "us-east-1".to_owned();
    let projection = RegionalProjection::new(
        StubProjection {
            snapshot: Ok(foreign),
        },
        Region::EuWest1,
    );
    assert_eq!(
        projection
            .snapshot(sample::<ApiKeyId>(5), workspace())
            .await,
        Err(ProjectionError::Unknown)
    );
}

#[tokio::test]
async fn a_key_row_and_a_placement_that_disagree_about_region_fail_closed() {
    let mut split = snapshot("active", KeyAuthorizationState::Active);
    split.key.region = "us-east-1".to_owned();
    let projection = RegionalProjection::new(
        StubProjection {
            snapshot: Ok(split),
        },
        Region::EuWest1,
    );
    assert_eq!(
        projection
            .snapshot(sample::<ApiKeyId>(5), workspace())
            .await,
        Err(ProjectionError::Unknown)
    );
}

#[tokio::test]
async fn a_placement_status_outside_the_vocabulary_is_never_guessed_at() {
    let projection = RegionalProjection::new(
        StubProjection {
            snapshot: Ok(snapshot("teleported", KeyAuthorizationState::Active)),
        },
        Region::EuWest1,
    );
    assert_eq!(
        projection
            .snapshot(sample::<ApiKeyId>(5), workspace())
            .await,
        Err(ProjectionError::Unavailable)
    );
}

#[tokio::test]
async fn an_absent_snapshot_is_unknown_and_an_unreadable_one_is_unavailable() {
    let absent = RegionalProjection::new(
        StubProjection {
            snapshot: Err(StoreError::Misconfigured {
                table: "regional-authz-projection".to_owned(),
            }),
        },
        Region::EuWest1,
    );
    assert_eq!(
        absent.snapshot(sample::<ApiKeyId>(5), workspace()).await,
        Err(ProjectionError::Unknown)
    );

    let unreadable = RegionalProjection::new(
        StubProjection {
            snapshot: Err(StoreError::Contended),
        },
        Region::EuWest1,
    );
    assert_eq!(
        unreadable
            .snapshot(sample::<ApiKeyId>(5), workspace())
            .await,
        Err(ProjectionError::Unavailable)
    );
}

// --- the credential itself --------------------------------------------------------

#[test]
fn a_credential_that_is_not_a_workspace_key_is_refused_before_anything_reads_it() {
    // A regional host accepts one credential. Refusing at construction means the
    // places that need the key id cannot each re-parse and disagree.
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
fn the_credential_binding_separates_two_credentials_and_discloses_neither() {
    let (one, _) = credential_pair(Region::EuWest1, 5);
    let (two, _) = credential_pair(Region::EuWest1, 6);
    assert_ne!(one.binding(), two.binding());
    // The binding is a handler's stable principal identity — it keys cursors and
    // replay records — so it must not be the digest a verifier is computed over.
    assert_ne!(&one.binding(), one.digest().as_bytes());
}

// --- the composed edge ------------------------------------------------------------

struct StubProjectionReader {
    snapshot: Result<ProjectedState, ProjectionError>,
}

#[async_trait]
impl ProjectionReader for StubProjectionReader {
    async fn snapshot(
        &self,
        _key: ApiKeyId,
        _workspace: WorkspaceId,
    ) -> Result<ProjectedState, ProjectionError> {
        self.snapshot.clone()
    }
}

struct FixedClock(i64);

impl EdgeClock for FixedClock {
    fn now(&self) -> Timestamp {
        moment(self.0)
    }
}

/// The projected state a current, correctly-provisioned key produces.
fn projected(account_state: AccountState, region: Region) -> ProjectedState {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    ProjectedState {
        epochs: ProjectedEpochs {
            key: Epoch::new(10),
            workspace: Epoch::new(30),
            account: Epoch::new(20),
        },
        workspace_id: workspace(),
        organization_id: raw(organization()),
        key_revoked: false,
        verifier: stored_for(&token, 1, PEPPER),
        audiences: AudienceSet::EMPTY.insert(AUDIENCE),
        scopes: ScopeSet::new(route(plain_route()).required_scope),
        account_state,
        region,
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
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

type Edge = RegionalEdge<StubProjectionReader, FixedClock>;

fn binding() -> EdgeBinding {
    EdgeBinding {
        audience: AUDIENCE,
        region: Region::EuWest1,
    }
}

/// Builds an edge over one snapshot answer.
fn edge(snapshot: Result<ProjectedState, ProjectionError>) -> Edge {
    RegionalEdge::new(
        ring(),
        StubProjectionReader { snapshot },
        FixedClock(NOW_MS + 2_000),
        binding(),
    )
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

#[tokio::test]
async fn a_current_credential_is_admitted_with_the_projected_authority() {
    let (token, key) = workspace_key(Region::EuWest1, 5);
    let credential = PresentedCredential::new(token.clone().into_bytes()).expect("a credential");
    let id = plain_route();
    let edge = edge(Ok(projected(AccountState::Active, Region::EuWest1)));
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
    // The named epoch record is the projection's three floors. `membership` stays
    // zero because a region holds no membership row to raise it.
    assert_eq!(context.auth.epochs.key, 10);
    assert_eq!(context.auth.epochs.workspace, 30);
    assert_eq!(context.auth.epochs.account, 20);
    assert_eq!(context.auth.epochs.membership, 0);
    assert_eq!(context.auth.credential_binding, credential.binding());
    assert_eq!(context.auth.scopes, ScopeSet::new(route(id).required_scope));
}

#[tokio::test]
async fn a_key_whose_verifier_does_not_match_is_refused() {
    // The whole authentication decision, in one case: the row exists, is current,
    // names this edge and carries the right scope — and the presented secret does
    // not produce its verifier.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.verifier = StoredVerifier::new([0; 32], 1);
    assert_eq!(
        admit(&edge(Ok(state)), id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );

    // And the neighbouring case: a verifier that is perfectly valid for a
    // *different* credential. A row cannot be made to admit the wrong key by
    // swapping which verifier it carries.
    let (other, _) = workspace_key(Region::EuWest1, 6);
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.verifier = stored_for(&other, 1, PEPPER);
    assert_eq!(
        admit(&edge(Ok(state)), id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[tokio::test]
async fn a_key_presented_to_the_wrong_audience_is_refused() {
    // One process can serve two audiences over one projection table. Without the
    // row naming which edges may accept it, a credential admitted at one would be
    // admitted at the other with the same MAC.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    for other in [
        AssertionAudience::RegionalSecret,
        AssertionAudience::RegionalObservation,
        AssertionAudience::RegionalOtlp,
        AssertionAudience::RegionalStream,
    ] {
        let mut state = projected(AccountState::Active, Region::EuWest1);
        state.audiences = AudienceSet::EMPTY.insert(other);
        assert_eq!(
            admit(&edge(Ok(state)), id, &headers(&token)).await.err(),
            Some(ErrorCode::Unauthenticated),
            "{other:?}"
        );
    }

    // A row that names no audience at all admits nothing rather than everything.
    let mut none = projected(AccountState::Active, Region::EuWest1);
    none.audiences = AudienceSet::EMPTY;
    assert_eq!(
        admit(&edge(Ok(none)), id, &headers(&token)).await.err(),
        Some(ErrorCode::Unauthenticated)
    );

    // A row that names this edge among others is admitted: the check is
    // membership, not equality.
    let mut both = projected(AccountState::Active, Region::EuWest1);
    both.audiences = AudienceSet::ALL;
    assert!(admit(&edge(Ok(both)), id, &headers(&token)).await.is_ok());
}

#[tokio::test]
async fn a_revoked_key_is_refused_before_its_secret_is_even_compared() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let mut revoked = projected(AccountState::Active, Region::EuWest1);
    revoked.key_revoked = true;
    assert_eq!(
        admit(&edge(Ok(revoked)), id, &headers(&token)).await.err(),
        Some(ErrorCode::TokenRevoked)
    );
}

#[tokio::test]
async fn a_rotation_does_not_invalidate_a_pre_rotation_credential() {
    // The region holds both halves of a rotation. A key whose row still names the
    // old version keeps working, and one re-peppered to the new version works
    // too. This is the case a ring that only held "the current pepper" would turn
    // into a total outage for every credential minted before the rotation.
    let rotated = parse_pepper_ring(
        PEPPER_SECRET,
        &pepper_document(&[(1, PEPPER), (2, [22; 32])]),
    )
    .expect("a usable ring");
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    for (version, pepper) in [(1_u16, PEPPER), (2, [22; 32])] {
        let mut state = projected(AccountState::Active, Region::EuWest1);
        state.verifier = stored_for(&token, version, pepper);
        let edge = RegionalEdge::new(
            rotated.clone(),
            StubProjectionReader {
                snapshot: Ok(state),
            },
            FixedClock(NOW_MS + 2_000),
            binding(),
        );
        assert!(
            admit(&edge, id, &headers(&token)).await.is_ok(),
            "pepper version {version} was refused"
        );
    }
}

#[tokio::test]
async fn a_pepper_version_this_region_never_loaded_is_retryable_and_not_a_refusal() {
    // The credential is unverifiable, not invalid. `401` would tell a customer to
    // rotate a key that was never wrong; `503` tells them, and us, the truth.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.verifier = stored_for(&token, 9, PEPPER);
    assert_eq!(
        admit(&edge(Ok(state)), id, &headers(&token)).await.err(),
        Some(ErrorCode::AuthenticationUnavailable)
    );
}

#[tokio::test]
async fn a_paused_account_loses_every_route_the_table_does_not_exempt() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    assert_eq!(
        admit(
            &edge(Ok(projected(AccountState::Paused, Region::EuWest1))),
            id,
            &headers(&token)
        )
        .await
        .err(),
        Some(ErrorCode::AccountPaused)
    );
}

#[tokio::test]
async fn a_workspace_placed_in_another_region_is_refused_here() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    assert_eq!(
        admit(
            &edge(Ok(projected(AccountState::Active, Region::UsEast1))),
            id,
            &headers(&token)
        )
        .await
        .err(),
        Some(ErrorCode::WrongWorkspaceRegion)
    );
}

#[tokio::test]
async fn a_key_minted_for_another_region_never_reaches_the_projection() {
    // The token's own region segment is checked at stage 1–2, before any row is
    // addressed, so a foreign key costs no read at all.
    let (token, _) = workspace_key(Region::UsEast1, 5);
    let id = plain_route();
    assert_eq!(
        admit(
            &edge(Err(ProjectionError::Unavailable)),
            id,
            &headers(&token)
        )
        .await
        .err(),
        Some(ErrorCode::WrongWorkspaceRegion)
    );
}

#[tokio::test]
async fn an_unavailable_projection_is_a_refusal_and_never_an_optimistic_admission() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    assert_eq!(
        admit(
            &edge(Err(ProjectionError::Unavailable)),
            id,
            &headers(&token)
        )
        .await
        .err(),
        Some(ErrorCode::AccountStateUnavailable)
    );
}

#[tokio::test]
async fn an_unknown_credential_and_an_unavailable_dependency_never_collapse() {
    // The requirement in one assertion: `401` and `503` are different answers and
    // a caller has to be able to tell them apart.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    assert_eq!(
        admit(&edge(Err(ProjectionError::Unknown)), id, &headers(&token))
            .await
            .err(),
        Some(ErrorCode::Unauthenticated)
    );
    assert_eq!(
        admit(
            &edge(Err(ProjectionError::Unavailable)),
            id,
            &headers(&token)
        )
        .await
        .err(),
        Some(ErrorCode::AccountStateUnavailable)
    );
}

#[tokio::test]
async fn a_credential_carrying_none_of_the_declared_scope_is_refused() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let mut state = projected(AccountState::Active, Region::EuWest1);
    state.scopes = ScopeSet::empty();
    assert_eq!(
        admit(&edge(Ok(state)), id, &headers(&token)).await.err(),
        Some(ErrorCode::InsufficientScope)
    );
}

#[tokio::test]
async fn a_request_with_no_credential_never_reaches_the_projection() {
    let id = plain_route();
    assert_eq!(
        admit(
            &edge(Err(ProjectionError::Unavailable)),
            id,
            &HeaderMap::new()
        )
        .await
        .err(),
        Some(ErrorCode::Unauthenticated)
    );
}

#[derive(Clone)]
struct MutableProjection {
    snapshot: std::sync::Arc<Mutex<Result<ProjectedState, ProjectionError>>>,
}

#[async_trait]
impl ProjectionReader for MutableProjection {
    async fn snapshot(
        &self,
        _key: ApiKeyId,
        _workspace: WorkspaceId,
    ) -> Result<ProjectedState, ProjectionError> {
        self.snapshot
            .lock()
            .expect("an uncontended fixture")
            .clone()
    }
}

#[tokio::test]
async fn a_long_lived_lease_observes_revocation_pause_audience_and_placement_change() {
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let snapshot = std::sync::Arc::new(Mutex::new(Ok(projected(
        AccountState::Active,
        Region::EuWest1,
    ))));
    let edge = RegionalEdge::new(
        ring(),
        MutableProjection {
            snapshot: std::sync::Arc::clone(&snapshot),
        },
        FixedClock(NOW_MS + 2_000),
        binding(),
    );
    let request_id = RequestId::parse("edge-fixture").expect("a request id");
    let request_headers = headers(&token);
    let authorization = edge
        .admit(&AdmissionRequest {
            request_id: &request_id,
            route: id,
            method: route(id).method,
            headers: &request_headers,
            body: &[],
        })
        .await
        .expect("the initial request is current")
        .auth;

    edge.revalidate(&authorization)
        .await
        .expect("unchanged authority renews the lease");

    // A revocation raises the key floor above the one the lease was opened at.
    let mut revoked = projected(AccountState::Active, Region::EuWest1);
    revoked.epochs.key = Epoch::new(11);
    *snapshot.lock().expect("an uncontended fixture") = Ok(revoked);
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::TokenRevoked)
    );

    // And so does a revoked key row, without moving any epoch.
    let mut flagged = projected(AccountState::Active, Region::EuWest1);
    flagged.key_revoked = true;
    *snapshot.lock().expect("an uncontended fixture") = Ok(flagged);
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::TokenRevoked)
    );

    // A key narrowed to no longer authorize this edge loses its open socket too.
    let mut narrowed = projected(AccountState::Active, Region::EuWest1);
    narrowed.audiences = AudienceSet::EMPTY.insert(AssertionAudience::RegionalSecret);
    *snapshot.lock().expect("an uncontended fixture") = Ok(narrowed);
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::TokenRevoked)
    );

    *snapshot.lock().expect("an uncontended fixture") =
        Ok(projected(AccountState::Paused, Region::EuWest1));
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::AccountPaused)
    );

    *snapshot.lock().expect("an uncontended fixture") =
        Ok(projected(AccountState::Active, Region::UsEast1));
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::WrongWorkspaceRegion)
    );

    *snapshot.lock().expect("an uncontended fixture") = Err(ProjectionError::Unavailable);
    assert_eq!(
        edge.revalidate(&authorization)
            .await
            .err()
            .map(|error| error.code),
        Some(ErrorCode::AccountStateUnavailable)
    );
}

/// A projection reader that counts how often the snapshot is consulted.
struct CountingProjection {
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    state: ProjectedState,
}

#[async_trait]
impl ProjectionReader for CountingProjection {
    async fn snapshot(
        &self,
        _key: ApiKeyId,
        _workspace: WorkspaceId,
    ) -> Result<ProjectedState, ProjectionError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.state.clone())
    }
}

#[tokio::test]
async fn one_snapshot_is_read_per_request_and_nothing_is_remembered_between_them() {
    // The revocation bound is now "the next request", and this is what makes that
    // true: no admission decision survives the request that produced it, so there
    // is no window inside which a revoked key is still admitted.
    let (token, _) = workspace_key(Region::EuWest1, 5);
    let id = plain_route();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let edge = RegionalEdge::new(
        ring(),
        CountingProjection {
            reads: std::sync::Arc::clone(&reads),
            state: projected(AccountState::Active, Region::EuWest1),
        },
        FixedClock(NOW_MS + 2_000),
        binding(),
    );

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
        "every request must read the projection"
    );
}

#[test]
fn the_fixture_route_set_is_drawn_from_the_generated_table() {
    // Guards the fixtures above: if the table ever stops declaring a scoped,
    // non-exempt regional read, the edge tests would silently test nothing.
    let id = plain_route();
    let descriptor = route(id);
    assert_eq!(descriptor.plane, Plane::Regional);
    assert!(descriptor.required_scope.is_some());
    assert!(!descriptor.pause_exempt);
    assert_eq!(descriptor.idempotency, IdempotencyKind::None);
}

#[test]
fn no_regional_source_can_reach_central_authz() {
    // The requirement stated as a property of the code rather than of a test
    // run: authentication is in-process, so this crate must hold no way to
    // invoke a function and no assertion exchange to invoke it with. The
    // manifest is checked as well as the sources, because a dependency nobody
    // imports today is a dependency somebody imports tomorrow.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("the manifest reads");
    assert!(
        !manifest.contains("aws-sdk-lambda.workspace"),
        "the crate must not link the Lambda SDK"
    );

    let mut checked = 0_usize;
    for entry in std::fs::read_dir(root.join("src")).expect("the source directory reads") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a source file reads");
        for forbidden in ["aws_sdk_lambda", "AssertionSource", "IssuedAssertion"] {
            assert!(
                !source.contains(forbidden),
                "`{}` still names `{forbidden}`",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(checked > 10, "only {checked} source files were scanned");
}
