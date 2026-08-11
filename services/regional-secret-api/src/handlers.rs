//! The provider-credential plaintext boundary, over the real adapters.
//!
//! The launch surface owns one write: register a provider credential. It seals
//! the API key under a minted credential identity and atomically commits the
//! backing custody rows, public binding, quota counter, and replay receipt.

use std::sync::Arc;

use aex_regional_http::context::RequestContext;
use aex_regional_http::idempotency::IdempotencyIdentity;
use aex_regional_http::mount::{UnaryDispatch, not_served};
use aex_regional_http::projection::{self, authority_failure};
use aex_regional_http::router::RouteOwner;
use aex_secret_aws::crypto::SecretCrypto;
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential as StoredCredential, SecretMetadata as StoredSecret,
    StoredGeneration,
};
use aex_secret_custody_dynamodb::expressions::{self, Quota};
use aex_secret_custody_dynamodb::store::SecretCustodyStore;
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SecretState, SourceGeneration};
use aex_secret_keystore_dynamodb::provision::BranchKeyAuthority;
use aex_session_dynamodb::error::{RetryPolicy, StoreError};
use aex_session_dynamodb::plan::Participant;
use aex_session_dynamodb::replay::{
    Backoff, DecodeReceipt, IdempotencyScope, RECEIPT_RETENTION, Receipt, ReceiptBody,
    ReceiptStore, ReplayRequest, commit_or_replay, key_digest,
};
use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::{PrefixedId as _, ProviderCredentialId, ResourceName, Uuid7, WorkspaceId};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    AcceptKind, Created, ProviderCredentialsApi, RequestContext as WireContext, WithETag,
    dispatch_provider_credentials,
};
use aex_wire::types::{Region, Timestamp};

/// The per-workspace provider-credential bound (D-13, owner batch B8).
///
/// It is far above any plausible legitimate use and far below the point where
/// the `SEC#{workspace}` partition becomes a problem.
/// A limit that has never bound anybody is the one to pick, and raising it later
/// is a constant edit with no migration behind it.
///
/// They are constants until the effective-limits producer exists;
/// `workspace_limit_get` and `workspace_limits_list` are themselves deferred, so
/// there is nothing authoritative to read yet. When that producer lands, this
/// becomes the default and the override arrives on `RequestContext.limits` with
/// no change to the expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretLimits {
    /// The largest number of provider-credential bindings one workspace may
    /// hold.
    pub max_provider_credentials: u64,
}

/// The accepted bounds.
#[must_use]
pub const fn secret_limits() -> SecretLimits {
    SecretLimits {
        max_provider_credentials: 100,
    }
}

/// The adapters and start-up bindings every request shares.
///
/// This is the value the router dispatches through, so it is where the
/// composition assertion belongs (D-4). Before this cluster it held
/// `{custody, custody_table}` while the crypto adapter was built and handed to a
/// `SecretEdge` the router never saw — which is why no handler could seal.
pub struct Shared {
    /// The custody authority.
    pub custody: Arc<dyn SecretCustodyStore>,
    /// The physical `regional-secret-custody` table name.
    pub custody_table: String,
    /// The envelope crypto over the **secret** root key.
    pub crypto: Arc<dyn SecretCrypto>,
    /// The workspace branch keys, created lazily on a first write (D-2).
    pub branch_keys: Arc<dyn BranchKeyAuthority>,
    /// Which plane a sealed value belongs to. Part of every encryption context.
    pub plane: Plane,
    /// Which region. Part of every encryption context.
    pub region: Region,
    /// The per-workspace collection bounds.
    pub limits: SecretLimits,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Shared")
            .field("custody_table", &self.custody_table)
            .field("plane", &self.plane)
            .field("region", &self.region)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// The receipt read side, over the custody authority.
struct CustodyReceipts<'a>(&'a dyn SecretCustodyStore);

#[async_trait::async_trait]
impl ReceiptStore for CustodyReceipts<'_> {
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        self.0
            .load_receipt(workspace, &scope.render(), &key_digest(key), now)
            .await
    }
}

/// Bounded, jittered waiting between contended attempts.
///
/// The ceiling is the policy's; the jitter is full jitter over `[0, backoff]`,
/// so two contending writers do not re-collide in lockstep.
struct SleepBackoff;

#[async_trait::async_trait]
impl Backoff for SleepBackoff {
    async fn wait(&self, policy: RetryPolicy, attempt: u32) {
        let ceiling = policy.backoff(attempt);
        if ceiling.is_zero() {
            return;
        }
        // A weak, dependency-free jitter source: the low bits of the monotonic
        // clock. This picks a wait, not a key.
        let spread = u128::from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos(),
        );
        let ceiling = ceiling.as_nanos().max(1);
        tokio::time::sleep(std::time::Duration::from_nanos(
            u64::try_from(spread % ceiling).unwrap_or(u64::MAX),
        ))
        .await;
    }
}

/// One committed registration response.
///
/// `ProviderCredential` publishes only the one-way `fingerprint`, so no key
/// material can enter a receipt through it either. What the receipt must carry
/// is the **minted `pcr_` id**: a retry inside the retention window has to
/// reproduce the original binding rather than mint a second one.
#[derive(Debug, Clone, PartialEq)]
struct RegisteredCredential(models::ProviderCredential);

impl RegisteredCredential {
    const KIND: &'static str = "ProviderCredential";
}

impl DecodeReceipt for RegisteredCredential {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != Self::KIND {
            return Err(StoreError::Invalid {
                detail: format!(
                    "a `{}` receipt cannot answer a credential registration",
                    receipt.response_kind
                ),
            });
        }
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(StoreError::Invalid {
                detail: "a credential receipt stores its response inline".to_owned(),
            });
        };
        serde_json::from_slice::<models::ProviderCredential>(bytes)
            .map(Self)
            .map_err(|error| StoreError::Invalid {
                detail: format!("a stored credential receipt did not decode: {error}"),
            })
    }
}

/// One request's view of the deployable.
pub struct Routes {
    shared: Arc<Shared>,
    cx: RequestContext,
}

impl Routes {
    /// Binds the shared adapters to one verified request.
    #[must_use]
    pub const fn new(shared: Arc<Shared>, cx: RequestContext) -> Self {
        Self { shared, cx }
    }

    /// Every route whose handler is complete, in `RouteId` order.
    ///
    /// Derived from the deferral ledger, not listed here: what this deployable
    /// owns and the contract does not defer. The rest is mounted as the
    /// generated refusal arm.
    #[must_use]
    pub fn served() -> Vec<RouteId> {
        RouteOwner::SecretApi
            .routes()
            .into_iter()
            .filter(|id| !route(*id).deferred)
            .collect()
    }

    fn now(&self) -> WireResult<Timestamp> {
        self.cx
            .now()
            .map_err(|_| WireError::new(ErrorCode::InternalError))
    }

    /// Performs the first seal.
    ///
    /// The whole operation is `SecretCrypto::seal` and nothing else — there is
    /// no new primitive here and there must not be one. "First seal" names an
    /// *ordering* property, not a distinct operation: it is the case where no
    /// source ciphertext exists to open, as against `rewrap`, which requires
    /// one. The AEAD, the context digest, the branch-key cache and the stored
    /// shape are shared verbatim with `rewrap` and `reveal`, and a second
    /// spelling would be a second place for the additional data to drift.
    async fn seal(
        &self,
        workspace: WorkspaceId,
        name: &ResourceName,
        generation: SourceGeneration,
        plaintext: &SecretPlaintext,
        now: Timestamp,
    ) -> WireResult<StoredGeneration> {
        let context = EncryptionContext {
            plane: self.shared.plane,
            region: self.shared.region,
            organization: self.cx.auth.organization_id,
            workspace,
            name: name.clone(),
            generation,
            // Session custody, not a source generation: a `rewrap` into a
            // session sets it, a first seal never does.
            custody_revision: None,
        };
        // Lazily created on this workspace's very first write, then read for
        // every later one. Exactly one extra KMS call and one extra transaction
        // per workspace, ever. A failure here is loud: a workspace with no
        // openable branch key has no seal path, and answering anything but a
        // refusal would mean writing a row nothing could ever open.
        let branch_key = self
            .shared
            .branch_keys
            .active_or_create(workspace)
            .await
            .map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?;
        let sealed = self
            .shared
            .crypto
            .seal(&context, &branch_key.wrapped_material, plaintext, now)
            .await
            .map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?;
        Ok(StoredGeneration {
            workspace,
            name: name.clone(),
            generation,
            // The wrapped branch key travels **inside** the row it sealed, so a
            // rotation between the read and the commit is harmless: a later
            // reveal unwraps the ciphertext this row carries, never the current
            // active one, and rotation only re-points `branch:ACTIVE` (D-5).
            ciphertext: sealed.to_ciphertext_ref(branch_key.hierarchy_version),
            context_digest: sealed.context_digest,
            created_at: now,
            revoked_at: None,
        })
    }

    /// Commits one credential registration under the request's replay identity.
    async fn commit_registration(
        &self,
        metadata: &StoredSecret,
        generation: &StoredGeneration,
        binding: &StoredCredential,
        value: &models::ProviderCredential,
        now: Timestamp,
    ) -> WireResult<models::ProviderCredential> {
        let identity = self.cx.idempotency.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InternalError)
                .with_message("this route requires a replay identity".to_owned())
        })?;
        // The scope subject is the **provider**, not the minted id: a retry has
        // to find the winner's receipt, and it cannot know the id the winner
        // minted.
        let scope = IdempotencyScope::new(
            "provider_credential.register",
            Some(binding.provider.as_str()),
        )
        .map_err(|error| {
            WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
        })?;
        let receipt = receipt_for(&scope, identity, RegisteredCredential::KIND, value, now)?;

        let custody = self.shared.custody.as_ref();
        let receipts = CustodyReceipts(custody);
        let outcome = commit_or_replay::<RegisteredCredential, _, _>(
            &receipts,
            &SleepBackoff,
            ReplayRequest {
                workspace: metadata.workspace,
                scope,
                key: &identity.key,
                intent: IntentDigest::from_bytes(identity.intent),
                receipt_participant: Participant::SECRET_IDEMPOTENCY,
                policy: RetryPolicy::PINNED,
                now,
            },
            || async {
                let plan = expressions::register_provider_credential(
                    &self.shared.custody_table,
                    generation,
                    metadata,
                    binding,
                    &receipt,
                    Quota::provider_credentials(self.shared.limits.max_provider_credentials),
                )?;
                custody.commit(&plan).await?;
                Ok(RegisteredCredential(value.clone()))
            },
        )
        .await
        .map_err(|error| write_failure(&error))?;
        Ok(outcome.into_inner().0)
    }
}

/// Builds the durable receipt for one accepted write.
///
/// A free function rather than a method: it depends on the request's replay
/// identity and the response it is recording, and on nothing about the
/// deployable.
fn receipt_for<T: serde::Serialize>(
    scope: &IdempotencyScope<'_>,
    identity: &IdempotencyIdentity,
    kind: &str,
    value: &T,
    now: Timestamp,
) -> WireResult<Receipt> {
    {
        let body = aex_wire::canonical::to_jcs_bytes(value).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        let expires_at = Timestamp::from_unix_millis(
            now.unix_millis()
                .saturating_add(i64::try_from(RECEIPT_RETENTION.as_millis()).unwrap_or(i64::MAX)),
        )
        .map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        Ok(Receipt {
            scope: scope.render(),
            key_sha256: key_digest(&identity.key),
            // Salted by the scope digest before it is stored (P0.3a), so the
            // durable `intentHash` of `{"value":"<the secret>"}` is not an
            // unsalted digest of a customer's secret that one precomputed table
            // could cover fleet-wide.
            intent: IntentDigest::from_bytes(identity.intent),
            response_kind: kind.to_owned(),
            response: ReceiptBody::Inline(body),
            committed_at: now,
            expires_at,
        })
    }
}

/// Renders a custody write failure on the wire.
///
/// A lost compare-and-swap is the caller's `precondition_failed`, never an
/// internal retry: an internal loop would hide a real concurrent writer, and the
/// route declares `precondition_failed` rather than `conflict` for exactly this.
///
/// The two counter participants are the only ones that map anywhere else: their
/// condition is a **quota**, so losing it is `limit_exceeded` — the code the
/// route declares — rather than a precondition the caller could have satisfied.
fn write_failure(error: &StoreError) -> WireError {
    match error {
        StoreError::IdempotencyConflict => WireError::new(ErrorCode::IdempotencyConflict),
        StoreError::PreconditionFailed { participant, .. }
            if *participant == Participant::SECRET_COUNT =>
        {
            WireError::new(ErrorCode::LimitExceeded).with_message(
                "this workspace holds the largest number of secrets it may hold".to_owned(),
            )
        }
        StoreError::PreconditionFailed { participant, .. }
            if *participant == Participant::CUSTODY_CREDENTIAL_COUNT =>
        {
            WireError::new(ErrorCode::LimitExceeded).with_message(
                "this workspace holds the largest number of provider credentials it may hold"
                    .to_owned(),
            )
        }
        other => authority_failure(other),
    }
}

impl std::fmt::Debug for Routes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Routes")
            .field("route", &self.cx.route)
            .finish_non_exhaustive()
    }
}

impl ProviderCredentialsApi for Routes {
    async fn provider_credential_get(
        &self,
        _cx: &WireContext,
        _provider_credential_id: ProviderCredentialId,
    ) -> WireResult<WithETag<models::ProviderCredential>> {
        Err(not_served(RouteId::ProviderCredentialGet))
    }

    /// `POST /api/workspace/provider-credentials` — register a BYOK key.
    ///
    /// The seal is the whole operation (D-12). **The key is not validated
    /// against the provider**: a probe call would put third-party latency and
    /// third-party availability inside a write path, would need an egress path
    /// from the one process that is otherwise network-isolated from providers,
    /// and the route declares **no error code** for "the provider rejected this
    /// key" — so the failure could only be reported as `invalid_request`, which
    /// would be a lie. A typo'd key is discovered at first use, as a provider
    /// authentication failure on a session, and that must be loud there.
    ///
    /// The minted secret name **is** the credential id's own string (D-10). It
    /// is not derived from the human label, because labels are not unique
    /// (D-11) and two registrations would collide on a name the route declares
    /// no error for; and it is not caller-supplied, because the generated
    /// request carries only `{provider, name, apiKey}` and a fourth field would
    /// make the customer responsible for a namespace they cannot see.
    async fn provider_credential_register(
        &self,
        _cx: &WireContext,
        body: models::ProviderCredentialRegisterRequest,
    ) -> WireResult<Created<models::ProviderCredential>> {
        let workspace = self.cx.auth.workspace_id;
        let now = self.now()?;
        let plaintext = SecretPlaintext::new(body.api_key.into_bytes()).map_err(|error| {
            WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
        })?;

        // Minted before the seal, because the credential id is the secret name
        // the encryption context has to name.
        let credential = ProviderCredentialId::from_uuid7(
            Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes()).map_err(|error| {
                WireError::new(ErrorCode::InternalError).with_message(error.to_string())
            })?,
        );
        let secret_name = ResourceName::parse(&credential.to_string()).map_err(|error| {
            WireError::new(ErrorCode::InternalError).with_message(error.to_string())
        })?;
        // Derived **before** the plaintext is dropped: this is the only moment
        // in the system's life when the value can be computed, because no read
        // path may ever hold the plaintext again.
        let fingerprint = plaintext.credential_fingerprint(workspace, credential);

        let generation = self
            .seal(
                workspace,
                &secret_name,
                SourceGeneration::FIRST,
                &plaintext,
                now,
            )
            .await?;
        drop(plaintext);

        let metadata = StoredSecret {
            workspace,
            name: secret_name.clone(),
            generation: SourceGeneration::FIRST,
            revision: SecretRevision::FIRST,
            state: SecretState::Ready,
            revocation_epoch: RevocationEpoch::INITIAL,
            revoked_through_revision: SecretRevision(0),
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let binding = StoredCredential {
            credential,
            workspace,
            name: body.name,
            provider: body.provider,
            secret_name,
            source_generation: SourceGeneration::FIRST,
            fingerprint,
            revision: 1,
            state: CredentialState::Ready,
            created_at: now,
            updated_at: now,
            revoked_at: None,
        };
        let value = projection::provider_credential(&binding);
        let committed = self
            .commit_registration(&metadata, &generation, &binding, &value, now)
            .await?;
        Ok(Created(committed))
    }

    async fn provider_credential_revoke(
        &self,
        _cx: &WireContext,
        _provider_credential_id: ProviderCredentialId,
        _body: models::EmptyRequest,
    ) -> WireResult<models::ProviderCredential> {
        Err(not_served(RouteId::ProviderCredentialRevoke))
    }

    async fn provider_credentials_list(
        &self,
        _cx: &WireContext,
        _query: models::ProviderCredentialsListQuery,
    ) -> WireResult<models::ProviderCredentialPage> {
        Err(not_served(RouteId::ProviderCredentialsList))
    }
}

#[async_trait::async_trait]
impl UnaryDispatch for Routes {
    fn owner(&self) -> RouteOwner {
        RouteOwner::SecretApi
    }

    fn served(&self) -> Vec<RouteId> {
        Self::served()
    }

    async fn dispatch(
        &self,
        cx: &RequestContext,
        accept: AcceptKind,
        raw: RawRequest<'_>,
        limits: RequestLimits,
    ) -> WireResult<RawResponse> {
        let wire = cx.to_wire(accept);
        let outcome = dispatch_provider_credentials(self, &wire, raw, limits).await?;
        match outcome {
            aex_wire::dispatch::DispatchOutcome::Unary(response) => Ok(response),
            aex_wire::dispatch::DispatchOutcome::Ndjson(never) => match never.0 {},
        }
    }
}

/// The mounted dispatcher: the shared adapters, plus a `Routes` per request.
///
/// [`Routes`] carries the request context because every authority write here is
/// workspace-scoped, so it cannot be the value `mount_unary` holds for the life
/// of the process. This is that value, and it costs one `Arc` clone per request.
///
/// It is published rather than written twice, once here and once in the `served`
/// target: two spellings of the composition would let the tested router and the
/// mounted router drift apart, which is the one thing the `served` target exists
/// to rule out.
#[derive(Debug)]
pub struct Dispatcher(Arc<Shared>);

impl Dispatcher {
    /// Binds the dispatcher to the shared adapters.
    #[must_use]
    pub const fn new(shared: Arc<Shared>) -> Self {
        Self(shared)
    }
}

#[async_trait::async_trait]
impl UnaryDispatch for Dispatcher {
    fn owner(&self) -> RouteOwner {
        RouteOwner::SecretApi
    }

    fn served(&self) -> Vec<RouteId> {
        Routes::served()
    }

    async fn dispatch(
        &self,
        cx: &RequestContext,
        accept: AcceptKind,
        raw: RawRequest<'_>,
        limits: RequestLimits,
    ) -> WireResult<RawResponse> {
        Routes::new(Arc::clone(&self.0), cx.clone())
            .dispatch(cx, accept, raw, limits)
            .await
    }
}
