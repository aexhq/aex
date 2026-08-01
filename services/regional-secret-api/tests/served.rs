//! The routes `regional-secret-api` genuinely serves, driven through the real
//! router.
//!
//! Both served routes are conditional updates on one metadata row, so what the
//! double records is the **expression** each one committed. A tombstone that did
//! not condition on the observed revision, or a second revoke that moved the
//! instant, would pass a body assertion and fail here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::projection::entity_tag;
use aex_regional_http::router::RouteOwner;
use aex_secret_custody_dynamodb::codec::SecretMetadata as StoredSecret;
use aex_secret_custody_dynamodb::store::{Page, SecretCustodyStore};
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
use aex_wire::types::{ETag, Region, RequestId, Timestamp};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use regional_secret_api::handlers::{Dispatcher, Routes, Shared};
use tower::ServiceExt as _;

const TABLE: &str = "dev-eu-west-1-regional-secret-custody";

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
        revision: SecretRevision(4),
        state: SecretState::Ready,
        revocation_epoch: RevocationEpoch(2),
        revoked_through_revision: SecretRevision(0),
        created_at: moment("2026-08-01T12:34:56.789Z"),
        updated_at: moment("2026-08-01T12:34:56.789Z"),
        revoked_at: None,
    }
}

/// One recorded conditional update.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Committed {
    participant: String,
    condition: String,
    update: String,
}

#[derive(Debug, Default)]
struct FakeCustody {
    secrets: BTreeMap<String, StoredSecret>,
    committed: Mutex<Vec<Committed>>,
}

impl FakeCustody {
    fn with(secrets: BTreeMap<String, StoredSecret>) -> Self {
        Self {
            secrets,
            committed: Mutex::new(Vec::new()),
        }
    }

    fn out_of_scope(name: &'static str) -> StoreError {
        StoreError::Misconfigured {
            table: name.to_owned(),
        }
    }

    fn writes(&self) -> Vec<Committed> {
        self.committed.lock().expect("an unpoisoned lock").clone()
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
        _workspace: WorkspaceId,
        _budget: PageBudget,
    ) -> Result<Vec<StoredSecret>, StoreError> {
        Err(Self::out_of_scope("secret.list"))
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
        _workspace: WorkspaceId,
        _budget: PageBudget,
    ) -> Result<Vec<aex_secret_custody_dynamodb::codec::ProviderCredential>, StoreError> {
        Err(Self::out_of_scope("credential.list"))
    }

    async fn load_provider_credential(
        &self,
        _workspace: WorkspaceId,
        _credential: ProviderCredentialId,
    ) -> Result<Option<aex_secret_custody_dynamodb::codec::ProviderCredential>, StoreError> {
        Err(Self::out_of_scope("credential.get"))
    }

    async fn page_secrets(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<Page<StoredSecret>, StoreError> {
        Err(Self::out_of_scope("secret.page"))
    }

    async fn page_provider_credentials(
        &self,
        _workspace: WorkspaceId,
        _budget: PageBudget,
        _after: Option<&PagePosition>,
    ) -> Result<Page<aex_secret_custody_dynamodb::codec::ProviderCredential>, StoreError> {
        Err(Self::out_of_scope("credential.page"))
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
        Err(Self::out_of_scope("custody.transaction"))
    }

    async fn commit_update(
        &self,
        builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        self.committed
            .lock()
            .expect("an unpoisoned lock")
            .push(Committed {
                participant: participant.to_string(),
                condition: built
                    .condition_expression()
                    .expect("every mutation is conditional")
                    .to_owned(),
                update: built.update_expression().to_owned(),
            });
        Ok(())
    }
}

fn context(request_id: RequestId, route_id: RouteId, if_match: Option<ETag>) -> RequestContext {
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
        if_match,
        received_at: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_754_051_696),
    }
}

struct Admit {
    if_match: Option<ETag>,
}

#[async_trait::async_trait]
impl EdgeAdmission for Admit {
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        Ok(context(
            request.request_id.clone(),
            request.route,
            self.if_match.clone(),
        ))
    }
}

fn router(custody: Arc<FakeCustody>, if_match: Option<ETag>) -> axum::Router {
    let shared = Arc::new(Shared {
        custody: custody as Arc<dyn SecretCustodyStore>,
        custody_table: TABLE.to_owned(),
    });
    mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit { if_match }),
        aex_wire::dispatch::RequestLimits::DEFAULT,
    )
    .expect("the served set mounts")
    .router
}

async fn send(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = if body.is_empty() {
        request.body(Body::empty())
    } else {
        request
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
    }
    .expect("a request");
    let response = router.clone().oneshot(request).await.expect("a response");
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

// --- the served set -----------------------------------------------------------------

#[test]
fn the_served_set_is_a_subset_of_the_owned_set() {
    let served = Routes::served();
    assert!(!served.is_empty());
    for id in &served {
        assert_eq!(
            aex_regional_http::router::route_owner(*id),
            Some(RouteOwner::SecretApi),
            "`{id}`"
        );
    }
}

#[test]
fn the_metadata_half_of_each_split_fragment_is_unreachable_here() {
    let served = Routes::served();
    for group in [RouteGroup::Secrets, RouteGroup::ProviderCredentials] {
        for id in RouteOwner::SessionApi.routes_in(group) {
            assert!(!served.contains(&id), "`{id}` is the session API's");
        }
    }
}

#[tokio::test]
async fn an_owned_but_unserved_route_is_absent_from_the_router() {
    let custody = Arc::new(FakeCustody::default());
    let router = router(custody, None);
    for id in RouteOwner::SecretApi.routes() {
        if Routes::served().contains(&id) {
            continue;
        }
        let descriptor = route(id);
        let (status, _) = send(
            &router,
            descriptor.method.as_str(),
            &descriptor.template.replace("{name}", "fixture"),
            "{}",
        )
        .await;
        assert!(
            matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "`{id}` answered {status} instead of being absent"
        );
    }
}

// --- delete -------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_tombstones_under_the_revision_the_caller_read() {
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored_secret("openai-key"),
    )])));
    let router = router(Arc::clone(&custody), None);
    let (status, _) = send(&router, "DELETE", "/api/workspace/secrets/openai-key", "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let writes = custody.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].participant, "secret.metadata");
    assert!(
        writes[0].condition.contains("revision = :expectedRevision"),
        "{:?}",
        writes[0]
    );
    assert!(
        writes[0].update.contains("#state = :deleted"),
        "{:?}",
        writes[0]
    );
}

/// The route declares no `not_found`, so an absent or already-tombstoned name is
/// a completed request — answered without a write, so a retry never advances the
/// revision a concurrent editor is fencing on.
#[tokio::test]
async fn a_repeated_delete_answers_no_content_without_writing_again() {
    let mut tombstoned = stored_secret("gone");
    tombstoned.state = SecretState::Deleted;
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "gone".to_owned(),
        tombstoned,
    )])));
    let router = router(Arc::clone(&custody), None);

    for _ in 0..2 {
        let (status, _) = send(&router, "DELETE", "/api/workspace/secrets/gone", "").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = send(&router, "DELETE", "/api/workspace/secrets/absent", "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        custody.writes().is_empty(),
        "an idempotent delete must not advance a revision"
    );
}

#[tokio::test]
async fn a_stale_if_match_refuses_the_delete_before_any_write() {
    let stored = stored_secret("openai-key");
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored,
    )])));
    let stale = ETag::parse("\"0000000000000000000000000000000000000000000000000000000000000000\"")
        .expect("a strong tag");
    let router = router(Arc::clone(&custody), Some(stale));
    let (status, body) = send(&router, "DELETE", "/api/workspace/secrets/openai-key", "").await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::PreconditionFailed.as_str())
    );
    assert!(custody.writes().is_empty(), "a refusal writes nothing");
}

#[tokio::test]
async fn the_current_entity_tag_satisfies_the_delete_precondition() {
    let stored = stored_secret("openai-key");
    let value = aex_regional_http::projection::secret_metadata(&stored).expect("projects");
    let current = entity_tag("SecretMetadata", &value).expect("a tag");
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored,
    )])));
    let router = router(Arc::clone(&custody), Some(current));
    let (status, _) = send(&router, "DELETE", "/api/workspace/secrets/openai-key", "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(custody.writes().len(), 1);
}

// --- revoke -------------------------------------------------------------------------

#[tokio::test]
async fn a_revoke_fences_every_prior_generation_and_answers_its_receipt() {
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored_secret("openai-key"),
    )])));
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "POST",
        "/api/workspace/secrets/openai-key/revocations",
        "{}",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let receipt: models::SecretRevocation =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(receipt.name.as_str(), "openai-key");
    assert_eq!(
        receipt.revision, 4,
        "a revocation advances no revision; it raises the fence"
    );

    let writes = custody.writes();
    assert_eq!(writes.len(), 1);
    assert!(
        writes[0]
            .condition
            .contains("revocationEpoch = :expectedEpoch"),
        "{:?}",
        writes[0]
    );
    assert!(
        writes[0]
            .update
            .contains("revokedThroughRevision = revision"),
        "the fence is what every use path compares against: {:?}",
        writes[0]
    );
}

/// The route declares an `Idempotency-Key`, and this handler honours it without
/// a durable receipt: revocation is terminal, so a replay is answered from the
/// stored row and writes nothing.
#[tokio::test]
async fn a_replayed_revoke_answers_the_stored_receipt_and_writes_nothing() {
    let mut fenced = stored_secret("openai-key");
    fenced.revoked_through_revision = fenced.revision;
    fenced.revoked_at = Some(moment("2026-08-01T14:00:00.000Z"));
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        fenced,
    )])));
    let router = router(Arc::clone(&custody), None);

    let mut receipts = Vec::new();
    for _ in 0..2 {
        let (status, body) = send(
            &router,
            "POST",
            "/api/workspace/secrets/openai-key/revocations",
            "{}",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        receipts.push(body);
    }
    assert_eq!(receipts[0], receipts[1], "a replay is byte-identical");
    assert_eq!(
        receipts[0]["revokedAt"].as_str(),
        Some("2026-08-01T14:00:00.000Z"),
        "the stored instant is answered, never a fresh one"
    );
    assert!(
        custody.writes().is_empty(),
        "a terminal record is never revoked twice"
    );
}

#[tokio::test]
async fn revoking_an_absent_or_deleted_secret_is_not_found() {
    let mut tombstoned = stored_secret("gone");
    tombstoned.state = SecretState::Deleted;
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "gone".to_owned(),
        tombstoned,
    )])));
    let router = router(Arc::clone(&custody), None);
    for name in ["gone", "absent"] {
        let (status, body) = send(
            &router,
            "POST",
            &format!("/api/workspace/secrets/{name}/revocations"),
            "{}",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}");
        assert_eq!(
            body["error"]["code"].as_str(),
            Some(ErrorCode::NotFound.as_str())
        );
    }
    assert!(custody.writes().is_empty());
}
