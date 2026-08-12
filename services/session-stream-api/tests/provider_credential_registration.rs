//! Provider-credential registration semantics inside `session-stream-api`.
//!
//! The doubles record the typed participants and whether every action is
//! conditional, not provider expression spelling. Exact `DynamoDB` request
//! construction belongs to the custody adapter's conformance tests.
//!
//! The crypto double is not a cipher and does not pretend to be one: the AEAD is
//! proved against real `KMS` in `aex-secret-aws`'s moto-backed target, where the
//! encryption context is authenticated additional data and a one-field change
//! fails the open. What is proved here is the property that lives here — that
//! the handler seals under a context naming the exact generation it is about to
//! commit, under the workspace's own branch key. The custody adapter's typed
//! codecs and conformance tests own the durable-row boundary.

use std::sync::{Arc, Mutex};

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::idempotency::{IdempotencyIdentity, IdentityContext};
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
use aex_wire::error::ErrorCode;
use aex_wire::idempotency::{IdempotencyKey, PrincipalScope};
use aex_wire::ids::{
    ApiKeyId, PrefixedId, ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::models;
use aex_wire::routes::RouteId;
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{Region, RequestId, Timestamp};
use session_stream_api::session::secret_registration::Registration;

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
    participants: Vec<Participant>,
    /// Per action, whether the participant committed under a condition.
    conditional: Vec<bool>,
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
                participants: plan.participants().to_vec(),
                conditional: plan
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
                            .or_else(|| {
                                action
                                    .delete()
                                    .and_then(|delete| delete.condition_expression())
                            })
                            .or_else(|| {
                                action.condition_check().map(
                                    aws_sdk_dynamodb::types::ConditionCheck::condition_expression,
                                )
                            })
                            .is_some()
                    })
                    .collect(),
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

fn registration(custody: Arc<FakeCustody>, crypto: Arc<FakeCrypto>) -> Registration {
    Registration::new(
        custody as Arc<dyn SecretCustodyStore>,
        TABLE.to_owned(),
        crypto as Arc<dyn SecretCrypto>,
        Arc::new(FakeBranchKeys),
        Plane::Dev,
        Region::EuWest1,
    )
}

fn request() -> models::ProviderCredentialRegisterRequest {
    serde_json::from_str(r#"{"apiKey":"sk-live-abcdefgh","name":"prod-key","provider":"openai"}"#)
        .expect("the generated request")
}

fn request_id() -> RequestId {
    RequestId::parse("credential-registration-fixture").expect("a request id")
}

// --- register: the credential binding transaction -----------------------------------

/// The whole shape of a registration: one transaction carrying the backing
/// secret, the directory entry, the receipt and the credential quota guard,
/// with the minted name equal to the credential id.
#[tokio::test]
async fn a_registration_commits_the_backing_secret_and_the_binding_together() {
    let custody = Arc::new(FakeCustody::default());
    let crypto = Arc::new(FakeCrypto::default());
    let registrar = registration(Arc::clone(&custody), Arc::clone(&crypto));
    let credential = registrar
        .register(
            &context(request_id(), RouteId::ProviderCredentialRegister),
            request(),
        )
        .await
        .expect("registration succeeds")
        .0;
    assert_eq!(credential.name.as_str(), "prod-key");
    assert_eq!(credential.provider, models::ProviderId::Openai);
    assert_eq!(credential.state, models::ProviderCredentialState::Ready);
    assert_eq!(credential.revision, 1);
    assert!(
        !serde_json::to_string(&credential)
            .expect("the response encodes")
            .contains("sk-live-abcdefgh"),
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
            Participant::SECRET_GENERATION,
            Participant::SECRET_METADATA,
            Participant::SECRET_LINEAGE,
            Participant::CUSTODY_PROVIDER_CREDENTIAL,
            Participant::SECRET_IDEMPOTENCY,
            Participant::CUSTODY_CREDENTIAL_COUNT,
        ]
    );
    assert!(
        transactions[0]
            .conditional
            .iter()
            .all(|conditional| *conditional),
        "every registration participant must remain conditional"
    );
}

/// D-11. Labels are not unique: registering one twice produces two bindings with
/// different ids and different fingerprints, and there is no collision to
/// report — which is consistent with the route declaring no collision error.
#[tokio::test]
async fn two_registrations_under_one_label_are_two_bindings() {
    let custody = Arc::new(FakeCustody::default());
    let registrar = registration(Arc::clone(&custody), Arc::new(FakeCrypto::default()));
    let mut ids = Vec::new();
    for _ in 0..2 {
        let credential = registrar
            .register(
                &context(request_id(), RouteId::ProviderCredentialRegister),
                request(),
            )
            .await
            .expect("registration succeeds")
            .0;
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
    let registrar = registration(Arc::clone(&custody), Arc::new(FakeCrypto::default()));
    let answered = registrar
        .register(
            &context(request_id(), RouteId::ProviderCredentialRegister),
            request(),
        )
        .await
        .expect("the replay succeeds")
        .0;
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
    let registrar = registration(Arc::clone(&custody), Arc::new(FakeCrypto::default()));
    let error = registrar
        .register(
            &context(request_id(), RouteId::ProviderCredentialRegister),
            request(),
        )
        .await
        .expect_err("the quota refuses the registration");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
}
