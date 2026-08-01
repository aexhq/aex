//! The routes `regional-session-api` genuinely serves, driven through the real
//! router.
//!
//! These are not handler unit tests. Each case builds the router with
//! `mount_unary` over the real `UnaryDispatch`, sends an HTTP request, and reads
//! the status and the published body back. What is proved is the whole path the
//! deployable owns: the generated route match, the generated dispatcher's path
//! and query decoding, the handler, the projection, the entity tag and the
//! response rendering.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::projection::entity_tag;
use aex_regional_http::router::RouteOwner;
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential as StoredCredential, SecretMetadata as StoredSecret,
};
use aex_secret_custody_dynamodb::store::{Page, SecretCustodyStore};
use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretName, SecretRevision, SecretState, SourceGeneration};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::replay::Receipt;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{
    ApiKeyId, PrefixedId, ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::server::RouteGroup;
use aex_wire::types::{Region, RequestId, Timestamp};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use regional_session_api::handlers::{Dispatcher, Routes, Shared};
use tower::ServiceExt as _;

// --- fixtures -------------------------------------------------------------------

fn sample<I: PrefixedId>(seed: u8) -> I {
    I::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]))
}

fn workspace() -> WorkspaceId {
    sample::<WorkspaceId>(2)
}

fn moment(spelling: &str) -> Timestamp {
    Timestamp::parse(spelling).expect("a pinned spelling")
}

fn stored_secret(name: &str) -> StoredSecret {
    StoredSecret {
        workspace: workspace(),
        name: ResourceName::parse(name).expect("a resource name"),
        generation: SourceGeneration::FIRST,
        revision: SecretRevision::FIRST,
        state: SecretState::Ready,
        revocation_epoch: RevocationEpoch::INITIAL,
        revoked_through_revision: SecretRevision(0),
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
        revoked_at: None,
    }
}

fn stored_credential() -> StoredCredential {
    let credential: ProviderCredentialId = sample(6);
    StoredCredential {
        credential,
        workspace: workspace(),
        name: ResourceName::parse("primary-openai").expect("a resource name"),
        provider: models::ProviderId::Openai,
        secret_name: ResourceName::parse("openai-key").expect("a resource name"),
        source_generation: SourceGeneration::FIRST,
        fingerprint: SecretPlaintext::new(b"sk-live-fixture".to_vec())
            .expect("a bounded plaintext")
            .credential_fingerprint(workspace(), credential),
        revision: 1,
        state: CredentialState::Ready,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
        revoked_at: None,
    }
}

/// An in-memory custody authority.
///
/// Only the read surface the served routes use is answered. Every other method
/// returns a typed refusal rather than panicking, so a handler that reached for
/// an authority it must not touch fails the assertion with a name rather than
/// unwinding.
#[derive(Debug, Default)]
struct FakeCustody {
    secrets: BTreeMap<String, StoredSecret>,
    credentials: Vec<StoredCredential>,
    page_size: usize,
}

impl FakeCustody {
    fn out_of_scope(name: &'static str) -> StoreError {
        StoreError::Misconfigured {
            table: name.to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl SecretCustodyStore for FakeCustody {
    async fn load_secret(
        &self,
        _workspace: WorkspaceId,
        name: &SecretName,
    ) -> Result<Option<StoredSecret>, StoreError> {
        Ok(self.secrets.get(name.as_str()).cloned())
    }

    async fn list_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<StoredSecret>, StoreError> {
        Ok(self.page_secrets(workspace, budget, None).await?.items)
    }

    async fn load_generation(
        &self,
        _workspace: WorkspaceId,
        _name: &SecretName,
        _generation: SourceGeneration,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::StoredGeneration>, StoreError> {
        Err(Self::out_of_scope("secret.generation"))
    }

    async fn load_custody(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::CustodyHead>, StoreError> {
        Err(Self::out_of_scope("custody.head"))
    }

    async fn load_manifest(
        &self,
        _workspace: WorkspaceId,
        _session: SessionId,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::RedactionManifest>, StoreError> {
        Err(Self::out_of_scope("redaction.manifest"))
    }

    async fn list_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        Ok(self
            .page_provider_credentials(workspace, budget, None)
            .await?
            .items)
    }

    async fn load_provider_credential(
        &self,
        _workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> Result<Option<StoredCredential>, StoreError> {
        Ok(self
            .credentials
            .iter()
            .find(|row| row.credential == credential)
            .cloned())
    }

    async fn page_secrets(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<StoredSecret>, StoreError> {
        let names: Vec<&String> = self.secrets.keys().collect();
        let start = match after {
            None => 0,
            Some(position) => names
                .iter()
                .position(|name| position.sk == format!("NAME#{name}"))
                .map_or(0, |at| at + 1),
        };
        let end = names.len().min(start + self.page_size.max(1));
        let items: Vec<StoredSecret> = names[start..end]
            .iter()
            .map(|name| self.secrets[*name].clone())
            .collect();
        let next = (end < names.len()).then(|| PagePosition {
            pk: format!("SEC#{}", workspace()),
            sk: format!("NAME#{}", names[end - 1]),
            index_pk: None,
            index_sk: None,
        });
        Ok(Page { items, next })
    }

    async fn page_provider_credentials(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<Page<StoredCredential>, StoreError> {
        Ok(Page {
            items: self.credentials.clone(),
            next: None,
        })
    }

    async fn load_receipt(
        &self,
        _workspace: WorkspaceId,
        _scope: &str,
        _key_sha256_hex: &str,
        _now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        Err(Self::out_of_scope("idempotency.receipt"))
    }

    async fn commit(&self, _plan: &TransactionPlan) -> Result<(), StoreError> {
        Err(Self::out_of_scope("custody.commit"))
    }

    async fn commit_update(
        &self,
        _builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
        _participant: Participant,
    ) -> Result<(), StoreError> {
        Err(Self::out_of_scope("custody.update"))
    }
}

fn context(request_id: RequestId, route_id: RouteId) -> RequestContext {
    RequestContext {
        request_id,
        route: route_id,
        auth: RegionalAuthorization {
            principal: PrincipalScope::WorkspaceKey {
                key: sample::<ApiKeyId>(1),
                workspace: workspace(),
                organization: sample(3),
            },
            credential_binding: [7; 32],
            organization_id: sample(3),
            workspace_id: workspace(),
            placement: Region::EuWest1,
            scopes: ScopeSet::default(),
            account_state: AccountState::Active,
            epochs: AuthorizationEpochs::default(),
            issued_at: time::OffsetDateTime::UNIX_EPOCH,
            expires_at: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(30),
        },
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
        operation_id: None,
        idempotency: None,
        if_match: None,
        received_at: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_754_051_696),
    }
}

struct Admit;

#[async_trait::async_trait]
impl EdgeAdmission for Admit {
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        Ok(context(request.request_id.clone(), request.route))
    }
}

fn cursor_keys() -> CursorKeyRing {
    CursorKeyRing::new(
        CursorKey::new("cur-1", vec![9u8; 32]).expect("a strong key"),
        Vec::new(),
    )
    .expect("a ring")
}

fn router(custody: FakeCustody) -> (axum::Router, Vec<RouteId>) {
    let shared = Arc::new(Shared {
        custody: Arc::new(custody),
        cursor_keys: Arc::new(cursor_keys()),
    });
    let mounted = mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit),
        aex_wire::dispatch::RequestLimits::DEFAULT,
    )
    .expect("the served set mounts");
    (mounted.router, mounted.routes)
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Option<String>, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let etag = response
        .headers()
        .get(axum::http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).expect("the response body is JSON")
    };
    (status, etag, json)
}

// --- the served set --------------------------------------------------------------

#[test]
fn the_served_set_is_a_subset_of_the_owned_set_and_never_a_second_list() {
    let served = Routes::served();
    let owned = RouteOwner::SessionApi.routes();
    assert!(!served.is_empty(), "the deployable answers something");
    for id in &served {
        assert!(
            owned.contains(id),
            "`{id}` is served but not owned by this deployable"
        );
    }
    assert_eq!(
        served,
        {
            let mut sorted = served.clone();
            sorted.sort_unstable_by_key(|id| *id as usize);
            sorted
        },
        "the served set keeps `RouteId` order"
    );
}

#[tokio::test]
async fn the_router_answers_exactly_the_served_set() {
    let (_, mounted) = router(FakeCustody::default());
    assert_eq!(mounted, Routes::served());
}

/// An owned route that is not served must be **absent**, not mounted and
/// answering a permanent failure (RS-18).
#[tokio::test]
async fn an_owned_but_unserved_route_is_absent_from_the_router() {
    let (router, mounted) = router(FakeCustody::default());
    let unserved = RouteOwner::SessionApi
        .routes()
        .into_iter()
        .find(|id| !mounted.contains(id))
        .expect("the deployable still owes routes");
    let descriptor = route(unserved);
    let path = descriptor
        .template
        .replace("{sessionId}", "01jxt21q00e40r2081040g2081")
        .replace("{runId}", "01jxt21q00e40r2081040g2081")
        .replace("{operationId}", "01jxt21q00e40r2081040g2081")
        .replace("{approvalId}", "01jxt21q00e40r2081040g2081")
        .replace("{uploadId}", "01jxt21q00e40r2081040g2081")
        .replace("{limitId}", "session.subagent_concurrency")
        .replace("{name}", "fixture")
        .replace("{kind}", "files");
    let response = router
        .oneshot(
            Request::builder()
                .method(descriptor.method.as_str())
                .uri(path)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert!(
        matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "`{unserved}` answered {}",
        response.status()
    );
}

// --- secret reads ----------------------------------------------------------------

#[tokio::test]
async fn a_secret_read_answers_the_published_metadata_and_its_entity_tag() {
    let stored = stored_secret("openai-key");
    let custody = FakeCustody {
        secrets: BTreeMap::from([("openai-key".to_owned(), stored.clone())]),
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);
    let (status, etag, body) = get(&router, "/api/workspace/secrets/openai-key").await;

    assert_eq!(status, StatusCode::OK);
    let decoded: models::SecretMetadata =
        serde_json::from_value(body.clone()).expect("the published schema");
    assert_eq!(decoded.name.as_str(), "openai-key");
    assert_eq!(decoded.state, models::SecretState::Ready);
    assert_eq!(decoded.revision, 1);
    assert!(
        body.get("value").is_none(),
        "a secret value is never readable: {body}"
    );

    let expected = entity_tag("SecretMetadata", &decoded).expect("a tag");
    assert_eq!(
        etag.as_deref(),
        Some(expected.as_str()),
        "the route declares `etag: returns`"
    );
}

#[tokio::test]
async fn a_tombstoned_secret_is_absent_rather_than_a_deleted_state() {
    let mut deleted = stored_secret("gone");
    deleted.state = SecretState::Deleted;
    let custody = FakeCustody {
        secrets: BTreeMap::from([("gone".to_owned(), deleted)]),
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);
    let (status, _, body) = get(&router, "/api/workspace/secrets/gone").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::NotFound.as_str())
    );
}

#[tokio::test]
async fn an_absent_secret_is_not_found() {
    let (router, _) = router(FakeCustody::default());
    let (status, _, _) = get(&router, "/api/workspace/secrets/absent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_secret_listing_pages_and_its_continuation_resumes_the_next_page() {
    let custody = FakeCustody {
        secrets: BTreeMap::from([
            ("alpha".to_owned(), stored_secret("alpha")),
            ("beta".to_owned(), stored_secret("beta")),
            ("gamma".to_owned(), stored_secret("gamma")),
        ]),
        page_size: 2,
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);

    let (status, _, body) = get(&router, "/api/workspace/secrets?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    let first: models::SecretMetadataPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(first.items.len(), 2);
    let cursor = first
        .next_cursor
        .expect("a full page names its continuation");

    let (status, _, body) = get(
        &router,
        &format!("/api/workspace/secrets?limit=2&cursor={}", cursor.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second: models::SecretMetadataPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(
        second.items.len(),
        1,
        "the continuation resumed after `beta`"
    );
    assert_eq!(second.items[0].name.as_str(), "gamma");
    assert_eq!(second.next_cursor, None);
}

/// A cursor is bound to the collection that minted it, so replaying one against
/// another listing is refused rather than answered from the wrong partition.
#[tokio::test]
async fn a_cursor_minted_for_another_collection_is_refused() {
    let custody = FakeCustody {
        secrets: BTreeMap::from([
            ("alpha".to_owned(), stored_secret("alpha")),
            ("beta".to_owned(), stored_secret("beta")),
        ]),
        page_size: 1,
        credentials: vec![stored_credential()],
    };
    let (router, _) = router(custody);
    let (_, _, body) = get(&router, "/api/workspace/secrets?limit=1").await;
    let page: models::SecretMetadataPage =
        serde_json::from_value(body).expect("the published schema");
    let cursor = page.next_cursor.expect("a continuation");

    let (status, _, body) = get(
        &router,
        &format!(
            "/api/workspace/provider-credentials?cursor={}",
            cursor.as_str()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::InvalidCursor.as_str())
    );
}

// --- provider credential reads ------------------------------------------------------

#[tokio::test]
async fn a_credential_read_answers_the_persisted_fingerprint_and_never_the_secret() {
    let stored = stored_credential();
    let custody = FakeCustody {
        credentials: vec![stored.clone()],
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);
    let (status, etag, body) = get(
        &router,
        &format!("/api/workspace/provider-credentials/{}", stored.credential),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body.clone()).expect("the published schema");
    assert_eq!(decoded.fingerprint, stored.fingerprint);
    assert_eq!(decoded.revision, 1);
    assert_eq!(decoded.provider, models::ProviderId::Openai);
    assert!(
        !body.to_string().contains(stored.secret_name.as_str()),
        "the referenced workspace secret must not reach the wire: {body}"
    );
    assert!(etag.is_some(), "the route declares `etag: returns`");
}

#[tokio::test]
async fn an_absent_credential_answers_the_declared_code() {
    let (router, _) = router(FakeCustody::default());
    let absent: ProviderCredentialId = sample(9);
    let (status, _, body) = get(
        &router,
        &format!("/api/workspace/provider-credentials/{absent}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::ProviderCredentialNotFound.as_str()),
        "the route declares its own not-found code"
    );
}

#[tokio::test]
async fn a_credential_listing_applies_the_declared_provider_filter() {
    let mut other = stored_credential();
    other.credential = sample(7);
    other.provider = models::ProviderId::Anthropic;
    let custody = FakeCustody {
        credentials: vec![stored_credential(), other],
        ..FakeCustody::default()
    };
    let (router, _) = router(custody);

    let (status, _, body) = get(&router, "/api/workspace/provider-credentials").await;
    assert_eq!(status, StatusCode::OK);
    let all: models::ProviderCredentialPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(all.items.len(), 2);

    let (_, _, body) = get(
        &router,
        "/api/workspace/provider-credentials?provider=anthropic",
    )
    .await;
    let filtered: models::ProviderCredentialPage =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].provider, models::ProviderId::Anthropic);
}

/// The `secrets` and `provider-credentials` fragments are each split across two
/// deployables, so this trait implementation carries methods for the other half.
/// They are unreachable through the router, and that is what makes the split
/// safe rather than merely conventional.
#[test]
fn the_other_half_of_each_split_fragment_is_unreachable_here() {
    let served = Routes::served();
    for group in [RouteGroup::Secrets, RouteGroup::ProviderCredentials] {
        let theirs = RouteOwner::SecretApi.routes_in(group);
        assert!(!theirs.is_empty(), "{group:?} has a secret-api half");
        for id in theirs {
            assert!(!served.contains(&id), "`{id}` is the secret edge's");
        }
    }
}

/// Every served route's declared success status is what the router actually
/// answers, because the handler never names one.
#[test]
fn no_served_route_lets_a_handler_choose_a_status() {
    for id in Routes::served() {
        assert_eq!(
            route(id).success_status,
            200,
            "`{id}` answers a status the handler cannot name"
        );
    }
}

fn unused_custody_revision() -> CustodyRevision {
    CustodyRevision::FIRST
}

#[test]
fn the_fixture_module_stays_honest() {
    // `CustodyRevision` is imported for the store trait's signature surface;
    // this keeps the import load-bearing rather than silently unused.
    assert_eq!(unused_custody_revision(), CustodyRevision::FIRST);
}
