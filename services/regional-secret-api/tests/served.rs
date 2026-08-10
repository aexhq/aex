//! The routes `regional-secret-api` genuinely serves, driven through the real
//! router.
//!
//! What the doubles record is the **expression** each write committed, not only
//! the body it answered. A tombstone that stopped conditioning on the observed
//! revision, a `secret_put` that lost its receipt participant, or a registration
//! that stopped writing its two halves in one transaction would each pass a body
//! assertion and fail here.
//!
//! The crypto double is not a cipher and does not pretend to be one: the AEAD is
//! proved against real `KMS` in `aex-secret-aws`'s moto-backed target, where the
//! encryption context is authenticated additional data and a one-field change
//! fails the open. What is proved here is the property that lives here — that
//! the handler seals under a context naming the exact generation it is about to
//! commit, under the workspace's own branch key, and that no plaintext reaches
//! any row but the sealed one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::idempotency::{IdempotencyIdentity, IdentityContext};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission, mount_unary};
use aex_regional_http::projection::entity_tag;
use aex_regional_http::router::RouteOwner;
use aex_secret_aws::crypto::{SealedSecret, SecretCrypto, SecretCryptoError};
use aex_secret_custody_dynamodb::codec::SecretMetadata as StoredSecret;
use aex_secret_custody_dynamodb::store::{Page, SecretCustodyStore};
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretName, SecretRevision, SecretState, SourceGeneration};
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
use aex_wire::types::{ETag, Region, RequestId, Timestamp};
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
    secrets: BTreeMap<String, StoredSecret>,
    receipt: Option<Receipt>,
    /// When set, every `commit` loses this participant's condition.
    lose: Option<Participant>,
    committed: Mutex<Vec<Committed>>,
    transacted: Mutex<Vec<Transacted>>,
}

impl FakeCustody {
    fn with(secrets: BTreeMap<String, StoredSecret>) -> Self {
        Self {
            secrets,
            ..Self::default()
        }
    }

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

    fn writes(&self) -> Vec<Committed> {
        self.committed.lock().expect("an unpoisoned lock").clone()
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
        route: RouteId::SecretPut,
        method: aex_wire::types::HttpMethod::Put,
    };
    let key = IdempotencyKey::parse("replay-key").expect("a replay key");
    aex_regional_http::idempotency::identity(&context, &key, body.as_bytes())
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
    sealing_router(custody, Arc::new(FakeCrypto::default()), if_match)
}

fn sealing_router(
    custody: Arc<FakeCustody>,
    crypto: Arc<FakeCrypto>,
    if_match: Option<ETag>,
) -> axum::Router {
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
        vec![
            RouteId::ProviderCredentialRegister,
            RouteId::SecretDelete,
            RouteId::SecretPut,
            RouteId::SecretRevoke
        ],
        "this deployable now serves every route it owns"
    );
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

/// Every owned route is now served, so this holds vacuously — which is the
/// point, and is why it is kept rather than deleted: if a fifth plaintext-bearing
/// route is ever authored under this owner, it starts life failing here.
#[tokio::test]
async fn an_owned_but_unserved_route_is_absent_from_the_router() {
    let custody = Arc::new(FakeCustody::default());
    let router = router(custody, None);
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

// --- register: the credential binding transaction -----------------------------------

/// The whole shape of a registration: one transaction carrying the backing
/// secret, the directory entry, the receipt and the credential quota guard,
/// with the minted name equal to the credential id.
#[tokio::test]
async fn a_registration_commits_the_backing_secret_and_the_binding_together() {
    let custody = Arc::new(FakeCustody::default());
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let (status, body) = send(
        &router,
        "POST",
        "/api/secrets/provider-credentials",
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
    let router = router(Arc::clone(&custody), None);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (status, body) = send(
            &router,
            "POST",
            "/api/secrets/provider-credentials",
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
        scope: "secret:credential:openai".to_owned(),
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
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "POST",
        "/api/secrets/provider-credentials",
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
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "POST",
        "/api/secrets/provider-credentials",
        r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::LimitExceeded.as_str())
    );
}

// --- put: the first seal ------------------------------------------------------------

/// The whole shape of a create, in one case: the seal names the generation the
/// transaction is about to commit, the plan carries its receipt and its quota
/// guard, the metadata is conditioned on absence, and the answer carries a tag
/// derived from the projected representation.
#[tokio::test]
async fn a_first_put_seals_under_the_generation_it_commits_and_answers_its_tag() {
    let custody = Arc::new(FakeCustody::default());
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let metadata: models::SecretMetadata =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(metadata.name.as_str(), "openai-key");
    assert_eq!(metadata.revision, 1, "the first set writes revision one");
    assert_eq!(metadata.state, models::SecretState::Ready);
    assert_eq!(metadata.revoked_at, None);

    let sealed = crypto.sealed();
    assert_eq!(sealed.len(), 1, "one read, one seal, one transaction");
    let (context, wrapped, length) = &sealed[0];
    assert_eq!(
        context.generation,
        SourceGeneration::FIRST,
        "the generation is inside the additional data, so it has to be known \
         before anything is sealed"
    );
    assert_eq!(context.name.as_str(), "openai-key");
    assert_eq!(context.workspace, workspace());
    assert_eq!(
        context.custody_revision, None,
        "a first seal is a source generation, never session custody"
    );
    assert_eq!(
        wrapped.as_slice(),
        &WRAPPED,
        "the workspace's own branch key"
    );
    assert_eq!(*length, "sk-live-abcdefgh".len());

    let transactions = custody.transactions();
    assert_eq!(transactions.len(), 1);
    assert_eq!(
        transactions[0].participants,
        [
            "secret.generation",
            "secret.metadata",
            "secret.lineage",
            "secret.idempotency",
            "secret.count",
        ],
        "the receipt and the quota guard ride the transaction they protect"
    );
    assert_eq!(
        transactions[0].conditions[1], "attribute_not_exists(pk)",
        "a create must lose to a concurrent create"
    );
    assert_eq!(
        transactions[0].conditions[3], "attribute_not_exists(pk)",
        "the conditional receipt put is the replay election"
    );
}

/// D-8. The write is always a compare-and-swap on the revision the handler read,
/// with or without an `If-Match`: the plan cannot be compiled without the
/// observed revision, so an absent precondition is never a blind overwrite.
#[tokio::test]
async fn a_replacement_conditions_on_the_revision_the_handler_read() {
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored_secret("openai-key"),
    )])));
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-rotated"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let metadata: models::SecretMetadata =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(metadata.revision, 5, "the observed revision was four");
    assert_eq!(
        crypto.sealed()[0].0.generation,
        SourceGeneration(2),
        "every accepted set mints a new generation, even for an unchanged value"
    );

    let transactions = custody.transactions();
    assert_eq!(
        transactions[0].conditions[1], "#state = :ready AND revision = :expectedRevision",
        "a replacement that stopped conditioning on the observed revision would \
         silently discard a concurrent write"
    );
    assert!(
        !transactions[0]
            .participants
            .contains(&"secret.count".to_owned()),
        "a replace does not consume quota; the collection does not grow"
    );
}

/// A lost compare-and-swap is `precondition_failed` and never an internal retry:
/// a retry loop would hide a real concurrent writer.
#[tokio::test]
async fn a_lost_compare_and_swap_is_a_precondition_failure_rather_than_a_retry() {
    let custody = Arc::new(FakeCustody::losing(Participant::SECRET_METADATA));
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::PreconditionFailed.as_str())
    );
    assert_eq!(
        custody.transactions().len(),
        1,
        "a domain precondition failure must never be retried into a second commit"
    );
}

/// D-13. The `(max + 1)`-th create answers the code the route declares, rather
/// than an unclassified internal error, and it commits nothing.
#[tokio::test]
async fn a_create_past_the_workspace_bound_answers_limit_exceeded() {
    let custody = Arc::new(FakeCustody::losing(Participant::SECRET_COUNT));
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::LimitExceeded.as_str()),
        "the declared code, not an internal error"
    );
}

/// A stale `If-Match` refuses before anything is sealed or written.
#[tokio::test]
async fn a_stale_if_match_refuses_the_put_before_any_seal() {
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored_secret("openai-key"),
    )])));
    let crypto = Arc::new(FakeCrypto::default());
    let stale = ETag::parse("\"0000000000000000000000000000000000000000000000000000000000000000\"")
        .expect("a strong tag");
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), Some(stale));
    let (status, _) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert!(crypto.sealed().is_empty(), "a refusal seals nothing");
    assert!(
        custody.transactions().is_empty(),
        "a refusal writes nothing"
    );
}

/// A `PUT` over a tombstone is refused. The route declares no `not_found`, so
/// the refusal is `precondition_failed`.
#[tokio::test]
async fn a_put_over_a_tombstone_is_refused_rather_than_resurrecting_it() {
    let mut tombstoned = stored_secret("gone");
    tombstoned.state = SecretState::Deleted;
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "gone".to_owned(),
        tombstoned,
    )])));
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let (status, _) = send(
        &router,
        "PUT",
        "/api/secrets/gone",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert!(crypto.sealed().is_empty());
}

/// D-8. A `PUT` over a **revoked** name succeeds and is the intended rotation
/// path: the fence stays where the revoke put it, and the new revision advances
/// past it, so the projection reports `ready` again while every session bound to
/// an earlier revision stays refused.
#[tokio::test]
async fn a_put_over_a_revoked_name_is_the_rotation_path_and_leaves_the_fence_standing() {
    let mut fenced = stored_secret("openai-key");
    fenced.revoked_through_revision = fenced.revision;
    fenced.revoked_at = Some(moment("2026-08-01T14:00:00.000Z"));
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        fenced.clone(),
    )])));
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-rotated"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let metadata: models::SecretMetadata =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(
        metadata.state,
        models::SecretState::Ready,
        "the new revision has advanced past the fence"
    );
    assert_eq!(metadata.revoked_at, None, "the set clears the instant");
    assert_eq!(
        metadata.revision,
        fenced.revision.next().0,
        "a session bound to revision {} stays refused",
        fenced.revision.0
    );
}

/// D-10. A caller who guesses a `pcr_` id gets the same answer whether or not
/// that credential exists, which leaks nothing and needs no special case.
#[tokio::test]
async fn a_put_on_a_reserved_credential_name_is_an_invalid_request() {
    let custody = Arc::new(FakeCustody::default());
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let reserved = ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [5; 10]));
    let (status, body) = send(
        &router,
        "PUT",
        &format!("/api/secrets/{reserved}"),
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::InvalidRequest.as_str())
    );
    assert!(crypto.sealed().is_empty(), "a reserved name seals nothing");
    assert!(custody.transactions().is_empty());
}

/// A live receipt replays the stored response and writes nothing.
#[tokio::test]
async fn a_replayed_put_answers_the_stored_response_without_a_second_seal() {
    let value = models::SecretMetadata {
        created_at: moment("2026-08-01T12:00:00.000Z"),
        name: ResourceName::parse("openai-key").expect("a resource name"),
        revision: 1,
        revoked_at: None,
        state: models::SecretState::Ready,
        updated_at: moment("2026-08-01T12:00:00.000Z"),
    };
    let stored = Receipt {
        scope: "secret:set:openai-key".to_owned(),
        key_sha256: "0".repeat(64),
        intent: aex_wire::idempotency::IntentDigest::from_bytes(identity("{}").intent),
        response_kind: "SecretMetadata".to_owned(),
        response: aex_session_dynamodb::replay::ReceiptBody::Inline(
            aex_wire::canonical::to_jcs_bytes(&value).expect("canonical bytes"),
        ),
        committed_at: moment("2026-08-01T12:00:00.000Z"),
        expires_at: moment("2036-08-01T12:00:00.000Z"),
    };
    let custody = Arc::new(FakeCustody::replaying(stored));
    let crypto = Arc::new(FakeCrypto::default());
    let router = sealing_router(Arc::clone(&custody), Arc::clone(&crypto), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answered: models::SecretMetadata =
        serde_json::from_value(body).expect("the published schema");
    assert_eq!(answered, value, "a replay is the winner's own response");
    assert!(
        custody.transactions().is_empty(),
        "a replay commits nothing at all"
    );
}

/// The same key under a different body is the conflict the route declares.
#[tokio::test]
async fn the_same_key_with_a_different_body_is_an_idempotency_conflict() {
    let value = models::SecretMetadata {
        created_at: moment("2026-08-01T12:00:00.000Z"),
        name: ResourceName::parse("openai-key").expect("a resource name"),
        revision: 1,
        revoked_at: None,
        state: models::SecretState::Ready,
        updated_at: moment("2026-08-01T12:00:00.000Z"),
    };
    let stored = Receipt {
        scope: "secret:set:openai-key".to_owned(),
        key_sha256: "0".repeat(64),
        // A different intent under the same key: the earlier winner asked for
        // something else.
        intent: aex_wire::idempotency::IntentDigest::from_bytes([9; 32]),
        response_kind: "SecretMetadata".to_owned(),
        response: aex_session_dynamodb::replay::ReceiptBody::Inline(
            aex_wire::canonical::to_jcs_bytes(&value).expect("canonical bytes"),
        ),
        committed_at: moment("2026-08-01T12:00:00.000Z"),
        expires_at: moment("2036-08-01T12:00:00.000Z"),
    };
    let custody = Arc::new(FakeCustody::replaying(stored));
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"]["code"].as_str(),
        Some(ErrorCode::IdempotencyConflict.as_str())
    );
    assert!(custody.transactions().is_empty());
}

/// No part of a secret may become durable outside the one sealed row.
#[tokio::test]
async fn no_plaintext_reaches_the_receipt_the_metadata_or_the_lineage() {
    let custody = Arc::new(FakeCustody::default());
    let router = router(Arc::clone(&custody), None);
    let (status, body) = send(
        &router,
        "PUT",
        "/api/secrets/openai-key",
        r#"{"value":"sk-live-abcdefgh"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.to_string().contains("sk-live-abcdefgh"),
        "the response echoed the value"
    );
    assert!(
        !format!("{:?}", custody.transactions()).contains("sk-live-abcdefgh"),
        "a durable row carried the plaintext"
    );
}

// --- delete -------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_tombstones_under_the_revision_the_caller_read() {
    let custody = Arc::new(FakeCustody::with(BTreeMap::from([(
        "openai-key".to_owned(),
        stored_secret("openai-key"),
    )])));
    let router = router(Arc::clone(&custody), None);
    let (status, _) = send(&router, "DELETE", "/api/secrets/openai-key", "").await;
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
        let (status, _) = send(&router, "DELETE", "/api/secrets/gone", "").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = send(&router, "DELETE", "/api/secrets/absent", "").await;
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
    let (status, body) = send(&router, "DELETE", "/api/secrets/openai-key", "").await;
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
    let (status, _) = send(&router, "DELETE", "/api/secrets/openai-key", "").await;
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
    let (status, body) = send(&router, "POST", "/api/secrets/openai-key/revocations", "{}").await;

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
        let (status, body) =
            send(&router, "POST", "/api/secrets/openai-key/revocations", "{}").await;
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
            &format!("/api/secrets/{name}/revocations"),
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
