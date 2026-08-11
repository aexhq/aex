//! The routes `regional-secret-api` genuinely serves, driven through the real
//! router.
//!
//! What the doubles record is the **expression** registration commits, not only
//! the body it answers. A registration that stopped writing its backing secret
//! and public binding in one transaction would pass a body assertion and fail
//! here.
//!
//! The crypto double is not a cipher and does not pretend to be one: the AEAD is
//! proved against real `KMS` in `aex-secret-aws`'s moto-backed target, where the
//! encryption context is authenticated additional data and a one-field change
//! fails the open. What is proved here is the property that lives here — that
//! the handler seals under a context naming the exact generation it is about to
//! commit, under the workspace's own branch key, and that no plaintext reaches
//! any row but the sealed one.

use std::sync::{Arc, Mutex};

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::idempotency::{IdempotencyIdentity, IdentityContext};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::router::RouteOwner;
use aex_secret_aws::crypto::{SealedSecret, SecretCrypto, SecretCryptoError};
use aex_secret_custody_dynamodb::codec::SecretMetadata as StoredSecret;
use aex_secret_custody_dynamodb::store::{Page, SecretCustodyStore};
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::secret::{SecretName, SourceGeneration};
use aex_secret_keystore_dynamodb::provision::{BranchKeyAuthority, ProvisionError};
use aex_secret_keystore_dynamodb::{ActiveBranchKey, BranchKeyId};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::replay::Receipt;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::{IdempotencyKey, PrincipalScope};
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
use regional_secret_api::handlers::{Dispatcher, Routes, Shared, secret_limits};
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

/// One recorded transaction, as the provider would have received it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transacted {
    participants: Vec<String>,
    /// Per action, the condition the participant committed under.
    conditions: Vec<String>,
    token: String,
}

#[derive(Debug, Default)]
struct FakeCustody {
    receipt: Option<Receipt>,
    /// When set, every `commit` loses this participant's condition.
    lose: Option<Participant>,
    transacted: Mutex<Vec<Transacted>>,
}

impl FakeCustody {
    fn replaying(receipt: Receipt) -> Self {
        Self {
            receipt: Some(receipt),
            ..Self::default()
        }
    }

    fn losing(participant: Participant) -> Self {
        Self {
            lose: Some(participant),
            ..Self::default()
        }
    }

    fn out_of_scope(name: &'static str) -> StoreError {
        StoreError::Misconfigured {
            table: name.to_owned(),
        }
    }

    fn transactions(&self) -> Vec<Transacted> {
        self.transacted.lock().expect("an unpoisoned lock").clone()
    }
}

#[async_trait::async_trait]
impl SecretCustodyStore for FakeCustody {
    async fn load_secret(
        &self,
        _workspace: WorkspaceId,
        _name: &SecretName,
    ) -> Result<Option<StoredSecret>, StoreError> {
        Err(Self::out_of_scope("secret.get"))
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
        Ok(self.receipt.clone())
    }

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        self.transacted
            .lock()
            .expect("an unpoisoned lock")
            .push(Transacted {
                participants: plan
                    .participants()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                conditions: plan
                    .actions()
                    .iter()
                    .map(|action| {
                        action
                            .put()
                            .and_then(|put| put.condition_expression())
                            .or_else(|| {
                                action
                                    .update()
                                    .and_then(|update| update.condition_expression())
                            })
                            .unwrap_or("<unconditional>")
                            .to_owned()
                    })
                    .collect(),
                token: plan.client_request_token().to_owned(),
            });
        match self.lose {
            Some(participant) => Err(StoreError::PreconditionFailed {
                participant,
                observed: None,
            }),
            None => Ok(()),
        }
    }

    async fn commit_update(
        &self,
        _builder: aws_sdk_dynamodb::types::builders::UpdateBuilder,
        _participant: Participant,
    ) -> Result<(), StoreError> {
        Err(Self::out_of_scope("secret.update"))
    }
}

/// A crypto double that records what it was asked to seal.
///
/// It is not a cipher and does not pretend to be one: the AEAD itself is proved
/// against real `KMS` in `aex-secret-aws`'s moto-backed target, where the
/// encryption context is authenticated additional data and a one-field change
/// fails the open. What this double proves is the property that lives *here* —
/// that the handler seals under a context naming the exact generation it is
/// about to commit, and under the workspace's own branch key.
#[derive(Debug, Default)]
struct FakeCrypto {
    sealed: Mutex<Vec<(EncryptionContext, Vec<u8>, usize)>>,
}

impl FakeCrypto {
    fn sealed(&self) -> Vec<(EncryptionContext, Vec<u8>, usize)> {
        self.sealed.lock().expect("an unpoisoned lock").clone()
    }
}

#[async_trait::async_trait]
impl SecretCrypto for FakeCrypto {
    async fn seal(
        &self,
        context: &EncryptionContext,
        wrapped_branch_key: &[u8],
        plaintext: &SecretPlaintext,
        _now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        self.sealed.lock().expect("an unpoisoned lock").push((
            context.clone(),
            wrapped_branch_key.to_vec(),
            plaintext.len(),
        ));
        Ok(SealedSecret {
            frame: vec![0xfe; 48],
            context_digest: context.digest(),
            wrapped_branch_key: wrapped_branch_key.to_vec(),
        })
    }

    async fn rewrap(
        &self,
        _sealed: &SealedSecret,
        _from: &EncryptionContext,
        _to: &EncryptionContext,
        _now: Timestamp,
    ) -> Result<SealedSecret, SecretCryptoError> {
        unreachable!("this deployable never rewraps")
    }

    async fn reveal(
        &self,
        _sealed: &SealedSecret,
        _context: &EncryptionContext,
        _now: Timestamp,
    ) -> Result<SecretPlaintext, SecretCryptoError> {
        unreachable!("this deployable never reveals")
    }
}

/// A branch-key authority that always has one.
#[derive(Debug)]
struct FakeBranchKeys;

const WRAPPED: [u8; 8] = [0xab; 8];

#[async_trait::async_trait]
impl BranchKeyAuthority for FakeBranchKeys {
    async fn active_or_create(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ActiveBranchKey, ProvisionError> {
        Ok(ActiveBranchKey {
            branch_key_id: BranchKeyId::of(workspace),
            version: Some("branch:version:fixture".to_owned()),
            create_time: "2026-08-01T00:00:00.000Z".to_owned(),
            kms_arn: "arn:aws:kms:eu-west-1:000000000000:key/secret".to_owned(),
            hierarchy_version: 3,
            wrapped_material: WRAPPED.to_vec(),
        })
    }
}

fn identity(body: &str) -> IdempotencyIdentity {
    let context = IdentityContext {
        principal: "key",
        organization: "org",
        workspace: workspace(),
        route: RouteId::ProviderCredentialRegister,
        method: aex_wire::types::HttpMethod::Post,
    };
    let key = IdempotencyKey::parse("replay-key").expect("a replay key");
    aex_regional_http::idempotency::identity(&context, &key, body.as_bytes())
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
        },
        limits: EffectiveLimits {
            json_body_bytes: 65_536,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
        operation_id: None,
        // The edge derives this for every route declaring an `Idempotency-Key`
        // and refuses the request without one, so a handler is entitled to it.
        idempotency: Some(identity("{}")),
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

fn router(custody: Arc<FakeCustody>) -> axum::Router {
    sealing_router(custody, Arc::new(FakeCrypto::default()))
}

fn sealing_router(custody: Arc<FakeCustody>, crypto: Arc<FakeCrypto>) -> axum::Router {
    let shared = Arc::new(Shared {
        custody: custody as Arc<dyn SecretCustodyStore>,
        custody_table: TABLE.to_owned(),
        crypto: crypto as Arc<dyn SecretCrypto>,
        branch_keys: Arc::new(FakeBranchKeys),
        plane: Plane::Dev,
        region: Region::EuWest1,
        limits: secret_limits(),
    });
    mount_unary(
        Arc::new(Dispatcher::new(shared)),
        Arc::new(Admit),
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
fn the_served_set_matches_the_generated_actual_mount_authority() {
    let registry: serde_json::Value = serde_json::from_str(include_str!(
        "../../../api/generated/registries/routes.json"
    ))
    .expect("generated route registry");
    let generated: Vec<RouteId> = registry["routes"]
        .as_array()
        .expect("route rows")
        .iter()
        .filter(|route| route["servedArtifact"] == "regional-secret-api")
        .map(|route| {
            RouteId::parse(route["operationId"].as_str().expect("operation id"))
                .expect("generated operation id")
        })
        .collect();
    assert_eq!(Routes::served(), generated);
    assert_eq!(
        generated,
        vec![RouteId::ProviderCredentialRegister],
        "this deployable now serves every route it owns"
    );
}

#[test]
fn the_credential_read_half_is_unreachable_here() {
    let served = Routes::served();
    for id in RouteOwner::SessionApi.routes_in(RouteGroup::ProviderCredentials) {
        assert!(!served.contains(&id), "`{id}` is the session API's");
    }
}

/// An owned route the contract defers is mounted and refuses honestly, rather
/// than being absent and leaving a caller unable to tell a published-but-unbuilt
/// operation from a mistyped path.
///
/// P3.3a and P3.3b mounted the last two, so this now holds vacuously — which is
/// the point, and is why it is kept rather than deleted: if a fifth
/// plaintext-bearing route is ever authored under this owner, it starts life
/// failing here.
#[tokio::test]
async fn an_owned_but_unserved_route_answers_the_published_refusal() {
    let custody = Arc::new(FakeCustody::default());
    let router = router(custody);
    let unserved: Vec<_> = RouteOwner::SecretApi
        .routes()
        .into_iter()
        .filter(|id| !Routes::served().contains(id))
        .collect();
    assert!(
        unserved.is_empty(),
        "`regional-secret-api` has unbuilt routes again: {unserved:?}"
    );
    for id in unserved {
        let descriptor = route(id);
        assert!(descriptor.deferred, "`{id}` is unserved and not deferred");
        let (status, body) = send(
            &router,
            descriptor.method.as_str(),
            &descriptor.template.replace("{name}", "fixture"),
            "{}",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "`{id}`");
        assert_eq!(body["error"]["code"], ErrorCode::NotImplemented.as_str());
    }
}

// --- register: the credential binding transaction -----------------------------------

/// The whole shape of a registration: one transaction carrying the backing
/// secret, the directory entry, the receipt and the credential quota guard,
/// with the minted name equal to the credential id.
#[tokio::test]
async fn a_registration_commits_the_backing_secret_and_the_binding_together() {
    let custody = Arc::new(FakeCustody::default());
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto));
    let (status, body) = send(
        &router,
        "POST",
        "/api/workspace/provider-credentials",
        r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let credential: models::ProviderCredential =
        serde_json::from_value(body.clone()).expect("the published schema");
    assert_eq!(credential.name.as_str(), "prod-key");
    assert_eq!(credential.provider, models::ProviderId::Openai);
    assert_eq!(credential.state, models::ProviderCredentialState::Ready);
    assert_eq!(credential.revision, 1);
    assert!(
        !body.to_string().contains("sk-live-abcdefgh"),
        "the response echoed the key"
    );

    let sealed = crypto.sealed();
    assert_eq!(sealed.len(), 1);
    assert_eq!(
        sealed[0].0.name.as_str(),
        credential.id.to_string(),
        "the minted secret name is the credential id's own string"
    );
    assert_eq!(sealed[0].0.generation, SourceGeneration::FIRST);

    let transactions = custody.transactions();
    assert_eq!(transactions.len(), 1, "a registration is one transaction");
    assert_eq!(
        transactions[0].participants,
        [
            "secret.generation",
            "secret.metadata",
            "secret.lineage",
            "custody.provider_credential",
            "secret.idempotency",
            "custody.credential_count",
        ]
    );
    for condition in &transactions[0].conditions[..5] {
        assert_eq!(
            condition, "attribute_not_exists(pk)",
            "a registration always creates, and a crash must leave neither half"
        );
    }
    assert_eq!(
        transactions[0].conditions[5], "attribute_not_exists(#count) OR #count < :max",
        "the quota guard is the one participant that is not a create"
    );
    assert!(
        !format!("{transactions:?}").contains("sk-live-abcdefgh"),
        "a durable row carried the key material"
    );
}

/// D-11. Labels are not unique: registering one twice produces two bindings with
/// different ids and different fingerprints, and there is no collision to
/// report — which is consistent with the route declaring no collision error.
#[tokio::test]
async fn two_registrations_under_one_label_are_two_bindings() {
    let custody = Arc::new(FakeCustody::default());
    let router = router(Arc::clone(&custody));
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (status, body) = send(
            &router,
            "POST",
            "/api/workspace/provider-credentials",
            r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let credential: models::ProviderCredential =
            serde_json::from_value(body).expect("the published schema");
        ids.push(credential.id);
    }
    assert_ne!(ids[0], ids[1], "the identity of a binding is its `pcr_` id");
    assert_eq!(custody.transactions().len(), 2);
}

/// A retry inside the retention window must reproduce the **original** id rather
/// than mint a second binding.
#[tokio::test]
async fn a_replayed_registration_answers_the_original_minted_identity() {
    let original = models::ProviderCredential {
        created_at: moment("2026-08-01T12:00:00.000Z"),
        fingerprint: aex_wire::ids::ContentHash::from_bytes([3; 32]),
        id: ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [7; 10])),
        name: ResourceName::parse("prod-key").expect("a resource name"),
        provider: models::ProviderId::Openai,
        revision: 1,
        revoked_at: None,
        state: models::ProviderCredentialState::Ready,
        updated_at: moment("2026-08-01T12:00:00.000Z"),
    };
    let stored = Receipt {
        scope: "provider_credential.register:openai".to_owned(),
        key_sha256: "0".repeat(64),
        intent: aex_wire::idempotency::IntentDigest::from_bytes(identity("{}").intent),
        response_kind: "ProviderCredential".to_owned(),
        response: aex_session_dynamodb::replay::ReceiptBody::Inline(
            aex_wire::canonical::to_jcs_bytes(&original).expect("canonical bytes"),
        ),
        committed_at: moment("2026-08-01T12:00:00.000Z"),
        expires_at: moment("2036-08-01T12:00:00.000Z"),
    };
    let custody = Arc::new(FakeCustody::replaying(stored));
    let router = router(Arc::clone(&custody));
    let (status, body) = send(
        &router,
        "POST",
        "/api/workspace/provider-credentials",
        r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let answered: models::ProviderCredential =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(
        answered.id, original.id,
        "a retry must reproduce the binding, not mint a second one"
    );
    assert!(custody.transactions().is_empty());
}

/// D-13, on the credential side. The declared `limit_exceeded` has a counter
/// behind it and the create commits nothing.
#[tokio::test]
async fn a_registration_past_the_credential_bound_answers_limit_exceeded() {
    let custody = Arc::new(FakeCustody::losing(Participant::CUSTODY_CREDENTIAL_COUNT));
    let router = router(Arc::clone(&custody));
    let (status, body) = send(
        &router,
        "POST",
        "/api/workspace/provider-credentials",
        r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::LimitExceeded.as_str())
    );
}
