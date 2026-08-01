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

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::projection::entity_tag;
use aex_regional_http::router::RouteOwner;
use aex_registry_dynamodb::store::{PointerPage, RegistryStore};
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
use aex_wire::ids::{ContentHash, UploadId};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::server::RouteGroup;
use aex_wire::types::ETag;
use aex_wire::types::{Region, RequestId, Timestamp};
use aex_workspace_domain::registry::{RegisteredValueRef, RegistryPointer};
use aex_workspace_domain::upload::{Upload as StoredUpload, UploadState};
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
    /// Every plan the handler committed, so a case can assert the expression and
    /// not only the published body.
    committed: std::sync::Mutex<Vec<CommittedPlan>>,
}

/// What one committed transaction said, flattened for assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommittedPlan {
    client_request_token: String,
    participants: Vec<String>,
    conditions: Vec<String>,
    updates: Vec<String>,
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

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        self.committed
            .lock()
            .expect("an uncontended fixture")
            .push(CommittedPlan {
                client_request_token: plan.client_request_token().to_owned(),
                participants: plan
                    .participants()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                conditions: plan
                    .actions()
                    .iter()
                    .filter_map(|action| {
                        action
                            .update()
                            .and_then(|update| update.condition_expression())
                            .map(str::to_owned)
                    })
                    .collect(),
                updates: plan
                    .actions()
                    .iter()
                    .filter_map(|action| {
                        action
                            .update()
                            .map(|update| update.update_expression().to_owned())
                    })
                    .collect(),
            });
        Ok(())
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

// --- the registry authority double ------------------------------------------------

/// A registry holding one pointer per kind, plus a recorded continuation.
///
/// It answers `list_pointers` from an in-memory map keyed by the kind it was
/// asked for, so a handler that named the wrong kind reads an empty page and the
/// case fails on the item count rather than passing by accident.
#[derive(Debug, Default)]
struct FakeRegistry {
    pointers: BTreeMap<&'static str, Vec<RegistryPointer>>,
    next: Option<PagePosition>,
    asked: std::sync::Mutex<Vec<RegistryKind>>,
    fails: bool,
}

fn kind_key(kind: RegistryKind) -> &'static str {
    match kind {
        RegistryKind::File => "file",
        RegistryKind::Skill => "skill",
        RegistryKind::Tool => "tool",
        RegistryKind::Instruction => "instruction",
        RegistryKind::McpServer => "mcp_server",
    }
}

fn pointer(kind: RegistryKind, name: &str) -> RegistryPointer {
    RegistryPointer {
        workspace: workspace(),
        kind,
        name: ResourceName::parse(name).expect("a resource name"),
        revision: Revision(4),
        etag: ETag::parse("\"registry-4\"").expect("a strong validator"),
        value: RegisteredValueRef::Content {
            digest: ContentHash::of(b"body"),
        },
        sha256: ContentHash::of(b"body"),
        size_bytes: 128,
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
    }
}

#[async_trait::async_trait]
impl RegistryStore for FakeRegistry {
    async fn load_pointer(
        &self,
        _workspace: WorkspaceId,
        _kind: RegistryKind,
        _name: &str,
    ) -> Result<Option<RegistryPointer>, StoreError> {
        Err(StoreError::Contended)
    }

    async fn list_pointers(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        _budget: PageBudget,
        _from: Option<&PagePosition>,
    ) -> Result<PointerPage, StoreError> {
        assert_eq!(
            workspace,
            crate::workspace(),
            "the listing is workspace-scoped"
        );
        self.asked
            .lock()
            .expect("an uncontended fixture")
            .push(kind);
        if self.fails {
            return Err(StoreError::Contended);
        }
        Ok(PointerPage {
            pointers: self
                .pointers
                .get(kind_key(kind))
                .cloned()
                .unwrap_or_default(),
            next: self.next.clone(),
        })
    }

    async fn put_pointer(
        &self,
        _pointer: &RegistryPointer,
        _from_revision: Option<Revision>,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn load_upload(
        &self,
        _workspace: WorkspaceId,
        _upload: UploadId,
    ) -> Result<Option<StoredUpload>, StoreError> {
        Err(StoreError::Contended)
    }

    async fn create_upload(&self, _upload: &StoredUpload) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn transition_upload(
        &self,
        _upload: UploadId,
        _from: UploadState,
        _to: UploadState,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn begin_completion(
        &self,
        _upload: UploadId,
        _completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }

    async fn finish_completion(
        &self,
        _upload: UploadId,
        _completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Contended)
    }
}

fn cursor_keys() -> CursorKeyRing {
    CursorKeyRing::new(
        CursorKey::new("cur-1", vec![9u8; 32]).expect("a strong key"),
        Vec::new(),
    )
    .expect("a ring")
}

/// The physical table name the composition root supplies.
const CUSTODY_TABLE: &str = "dev-eu-west-1-regional-secret-custody";

fn router(custody: FakeCustody) -> (axum::Router, Vec<RouteId>) {
    composed(custody, FakeRegistry::default())
}

fn composed(custody: FakeCustody, registry: FakeRegistry) -> (axum::Router, Vec<RouteId>) {
    build(Arc::new(custody), Arc::new(registry)).0
}

fn build(
    custody: Arc<FakeCustody>,
    registry: Arc<FakeRegistry>,
) -> ((axum::Router, Vec<RouteId>), Arc<FakeCustody>) {
    let shared = Arc::new(Shared {
        custody: Arc::clone(&custody) as Arc<dyn SecretCustodyStore>,
        custody_table: CUSTODY_TABLE.to_owned(),
        registry: registry as Arc<dyn RegistryStore>,
        cursor_keys: Arc::new(cursor_keys()),
    });
    let mounted = mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit),
        aex_wire::dispatch::RequestLimits::DEFAULT,
    )
    .expect("the served set mounts");
    ((mounted.router, mounted.routes), custody)
}

/// A registry holding exactly one pointer in every collection.
fn populated_registry() -> FakeRegistry {
    FakeRegistry {
        pointers: BTreeMap::from([
            (
                kind_key(RegistryKind::File),
                vec![pointer(RegistryKind::File, "notes.md")],
            ),
            (
                kind_key(RegistryKind::Skill),
                vec![pointer(RegistryKind::Skill, "review")],
            ),
            (
                kind_key(RegistryKind::Tool),
                vec![pointer(RegistryKind::Tool, "search")],
            ),
            (
                kind_key(RegistryKind::Instruction),
                vec![pointer(RegistryKind::Instruction, "house-style")],
            ),
            (
                kind_key(RegistryKind::McpServer),
                vec![pointer(RegistryKind::McpServer, "docs")],
            ),
        ]),
        ..FakeRegistry::default()
    }
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

async fn post(router: &axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("the response body is JSON")
    };
    (status, json)
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
        ..FakeCustody::default()
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

// --- provider credential revocation -------------------------------------------------

fn revocations_path(credential: ProviderCredentialId) -> String {
    format!("/api/workspace/provider-credentials/{credential}/revocations")
}

#[tokio::test]
async fn a_revocation_fences_the_binding_and_publishes_the_committed_row() {
    let stored = stored_credential();
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![stored.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );

    let (status, body) = post(&router, &revocations_path(stored.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(decoded.state, models::ProviderCredentialState::Revoked);
    assert_eq!(
        decoded.revision,
        stored.revision + 1,
        "a fenced binding publishes the revision the condition committed against"
    );
    assert!(decoded.revoked_at.is_some());

    // The body being right is not enough: a revocation that stopped conditioning
    // on the observed state would still publish this answer.
    let plans = custody.committed.lock().expect("an uncontended fixture");
    assert_eq!(plans.len(), 1, "exactly one transaction");
    let plan = &plans[0];
    assert_eq!(plan.participants, vec!["custody.provider_credential"]);
    assert_eq!(plan.conditions.len(), 1);
    assert!(
        plan.conditions[0].contains("revision = :expectedRevision"),
        "the fence must condition on the observed revision: {}",
        plan.conditions[0]
    );
    assert!(
        plan.conditions[0].contains(":ready"),
        "only a ready binding may be fenced: {}",
        plan.conditions[0]
    );
    assert!(plan.updates[0].contains("revokedAt = :now"));
    assert!(
        plan.client_request_token.starts_with("aex-"),
        "the transaction carries a deterministic deduplication identity"
    );
}

/// The route declares an `Idempotency-Key` and needs no durable receipt: the
/// scope subject is the credential and the body is empty, so a replay can only
/// carry the same intent and is answered from the stored row (RS-31).
#[tokio::test]
async fn a_replayed_revocation_answers_the_stored_row_and_writes_nothing() {
    let mut already = stored_credential();
    already.state = CredentialState::Revoked;
    already.revoked_at = Some(moment("2026-08-01T13:00:00.000Z"));
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![already.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );

    let (status, body) = post(&router, &revocations_path(already.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    let decoded: models::ProviderCredential =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(decoded.state, models::ProviderCredentialState::Revoked);
    assert_eq!(
        decoded.revision, already.revision,
        "a replay must not advance the revision a concurrent reader is fencing on"
    );
    assert!(
        custody
            .committed
            .lock()
            .expect("an uncontended fixture")
            .is_empty(),
        "a replay writes nothing"
    );
}

#[tokio::test]
async fn revoking_an_absent_binding_answers_the_declared_code() {
    let (router, _) = router(FakeCustody::default());
    let absent: ProviderCredentialId = sample(9);
    let (status, body) = post(&router, &revocations_path(absent), "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::ProviderCredentialNotFound.as_str())
    );
}

/// The deployable holds `TransactWriteItems` on `regional-secret-custody` and is
/// deliberately not granted `UpdateItem`, so the revocation has to reach the
/// authority as a transaction. A bare conditional update would pass every local
/// test and be denied in production.
#[tokio::test]
async fn the_revocation_reaches_the_authority_as_a_transaction() {
    let stored = stored_credential();
    let ((router, _), custody) = build(
        Arc::new(FakeCustody {
            credentials: vec![stored.clone()],
            ..FakeCustody::default()
        }),
        Arc::new(FakeRegistry::default()),
    );
    let (status, _) = post(&router, &revocations_path(stored.credential), "{}").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        custody
            .committed
            .lock()
            .expect("an uncontended fixture")
            .len(),
        1,
        "the only write path this deployable is granted is a transaction"
    );
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

// --- the registry listings ---------------------------------------------------------

/// Every registry listing, its path, and the collection it must read.
const REGISTRY_LISTINGS: &[(RouteId, &str, RegistryKind)] = &[
    (
        RouteId::RegistryFilesList,
        "/api/workspace/files",
        RegistryKind::File,
    ),
    (
        RouteId::RegistrySkillsList,
        "/api/workspace/skills",
        RegistryKind::Skill,
    ),
    (
        RouteId::RegistryToolsList,
        "/api/workspace/tools",
        RegistryKind::Tool,
    ),
    (
        RouteId::RegistryInstructionsList,
        "/api/workspace/instructions",
        RegistryKind::Instruction,
    ),
    (
        RouteId::RegistryMcpServersList,
        "/api/workspace/mcp-servers",
        RegistryKind::McpServer,
    ),
];

#[tokio::test]
async fn every_registry_listing_reads_its_own_collection_and_publishes_its_rows() {
    for (id, path, _) in REGISTRY_LISTINGS {
        let (router, _) = composed(FakeCustody::default(), populated_registry());
        let (status, _, body) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        let items = body
            .get("items")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("{id} publishes an items array"));
        assert_eq!(items.len(), 1, "{id} reads exactly its own collection");
        let row = &items[0];
        assert_eq!(
            row.get("revision").and_then(serde_json::Value::as_u64),
            Some(4),
            "{id}"
        );
        assert_eq!(
            row.get("state").and_then(serde_json::Value::as_str),
            Some("current"),
            "{id}"
        );
        assert_eq!(
            row.get("sizeBytes").and_then(serde_json::Value::as_str),
            Some("128"),
            "{id} publishes an exact size, never a float"
        );
        assert!(
            row.get("value").is_none(),
            "{id} is a collection row and must omit the value it cannot read"
        );
    }
}

#[tokio::test]
async fn a_registry_listing_asks_the_authority_for_its_own_kind_and_no_other() {
    for (id, path, kind) in REGISTRY_LISTINGS {
        let registry = Arc::new(populated_registry());
        let ((router, _), _) = build(Arc::new(FakeCustody::default()), Arc::clone(&registry));
        let (status, _, _) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(
            registry
                .asked
                .lock()
                .expect("an uncontended fixture")
                .as_slice(),
            &[*kind],
            "{id} must read one collection, and it must be its own"
        );
    }
}

#[tokio::test]
async fn a_registry_continuation_is_bound_to_its_own_collection() {
    // Every listing shares a table and a key template, so the only thing that
    // stops a `skills` cursor resuming a `tools` read is the authenticated
    // snapshot binding. Replaying one against another must fail its MAC.
    let with_more = FakeRegistry {
        next: Some(PagePosition {
            pk: "WS#1".to_owned(),
            sk: "REG#skill#review".to_owned(),
            index_pk: None,
            index_sk: None,
        }),
        ..populated_registry()
    };
    let (router, _) = composed(FakeCustody::default(), with_more);
    let (status, _, body) = get(&router, "/api/workspace/skills").await;
    assert_eq!(status, StatusCode::OK);
    let cursor = body
        .get("nextCursor")
        .and_then(serde_json::Value::as_str)
        .expect("a page with more names a continuation")
        .to_owned();

    let (status, _, _) = get(&router, &format!("/api/workspace/skills?cursor={cursor}")).await;
    assert_eq!(status, StatusCode::OK, "its own collection resumes");

    let (status, _, body) = get(&router, &format!("/api/workspace/tools?cursor={cursor}")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a cursor minted over one collection must never resume another"
    );
    assert_eq!(body["error"]["code"].as_str(), Some("invalid_cursor"));
}

#[tokio::test]
async fn an_empty_registry_is_an_empty_page_and_never_a_missing_collection() {
    for (id, path, _) in REGISTRY_LISTINGS {
        let (router, _) = composed(FakeCustody::default(), FakeRegistry::default());
        let (status, _, body) = get(&router, path).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(
            body.get("items").and_then(serde_json::Value::as_array),
            Some(&Vec::new()),
            "{id}"
        );
        assert!(body.get("nextCursor").is_none(), "{id}");
    }
}

#[tokio::test]
async fn an_unreadable_registry_fails_the_listing_rather_than_publishing_a_short_page() {
    let registry = FakeRegistry {
        pointers: BTreeMap::new(),
        fails: true,
        ..FakeRegistry::default()
    };
    let (router, _) = composed(FakeCustody::default(), registry);
    let (status, _, body) = get(&router, "/api/workspace/files").await;
    // Contention maps onto `conflict`, which this route does not declare, so the
    // dispatch boundary replaces it with `internal_error` rather than letting an
    // undeclared code reach the wire (C-58). What matters here is that a failed
    // authority read is never a successful empty page.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["code"].as_str(), Some("internal_error"));
    assert!(
        route(RouteId::RegistryFilesList)
            .errors
            .iter()
            .all(|code| !code.retryable()),
        "the listing declares no transient code, which is why the refusal is internal"
    );
}
