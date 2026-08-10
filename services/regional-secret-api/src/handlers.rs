//! The plaintext-boundary half of `regional:secrets`, over the real adapters.
//!
//! This deployable owns the four routes that carry or destroy key material.
//! `secret_delete` and `secret_revoke` need no cryptography at all — a tombstone
//! and a revocation fence are conditional updates on one metadata row, and
//! neither reads or writes a ciphertext. `secret_put` performs the platform's
//! **first seal**: it reads the current metadata, derives the next generation,
//! seals the plaintext under an encryption context that names that exact
//! generation, and commits one transaction.

use std::sync::Arc;

use aex_regional_http::context::RequestContext;
use aex_regional_http::idempotency::IdempotencyIdentity;
use aex_regional_http::mount::{UnaryDispatch, not_served};
use aex_regional_http::projection::{self, ProjectionError, authority_failure, entity_tag};
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
    AcceptKind, Created, NoContent, ProviderCredentialsApi, RequestContext as WireContext,
    SecretsApi, WithETag, dispatch_provider_credentials, dispatch_secrets,
};
use aex_wire::types::{ETag, Region, Timestamp};

/// The per-workspace collection bounds (D-13, owner batch B8).
///
/// Both are far above any plausible legitimate use and far below the point where
/// the `SEC#{workspace}` partition or a listing's page budget becomes a problem.
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
    /// The largest number of secrets one workspace may hold.
    pub max_secrets: u64,
    /// The largest number of provider-credential bindings one workspace may
    /// hold.
    pub max_provider_credentials: u64,
}

/// The accepted bounds.
#[must_use]
pub const fn secret_limits() -> SecretLimits {
    SecretLimits {
        max_secrets: 500,
        max_provider_credentials: 100,
    }
}

/// Whether a secret name belongs to the reserved credential namespace (D-10).
///
/// The reservation is **typed**, not a string-prefix convention: a name is
/// reserved exactly when it parses as a `ProviderCredentialId`. A prefix check
/// would reserve names no registration could ever mint and would still have to
/// parse to be sure, so the parse is the whole rule.
#[must_use]
pub fn is_reserved_name(name: &ResourceName) -> bool {
    ProviderCredentialId::parse(name.as_str()).is_ok()
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

/// One committed `secret_put` response, so `commit_or_replay` can rebuild it.
///
/// The receipt stores the **canonical response body** inline. `SecretMetadata`
/// is `{name, revision, state, createdAt, updatedAt, revokedAt}` and has no
/// field that could hold key material, so no plaintext, ciphertext, wrapped key
/// or context digest can enter a receipt through it.
#[derive(Debug, Clone, PartialEq)]
struct SealedMetadata(models::SecretMetadata);

impl SealedMetadata {
    const KIND: &'static str = "SecretMetadata";
}

impl DecodeReceipt for SealedMetadata {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != Self::KIND {
            return Err(StoreError::Invalid {
                detail: format!(
                    "a `{}` receipt cannot answer a secret write",
                    receipt.response_kind
                ),
            });
        }
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(StoreError::Invalid {
                detail: "a secret receipt stores its response inline".to_owned(),
            });
        };
        serde_json::from_slice::<models::SecretMetadata>(bytes)
            .map(Self)
            .map_err(|error| StoreError::Invalid {
                detail: format!("a stored secret receipt did not decode: {error}"),
            })
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

    /// Enforces the caller's `If-Match` against the current representation.
    ///
    /// The comparison is byte-for-byte against the tag the read path would have
    /// issued, so a caller cannot pass a precondition by presenting a tag from
    /// another resource, another revision or another representation.
    fn check_if_match(&self, stored: &StoredSecret) -> WireResult<()> {
        let Some(presented) = self.cx.if_match.as_ref() else {
            return Ok(());
        };
        let value = projection::secret_metadata(stored).map_err(WireError::from)?;
        let current: ETag = entity_tag("SecretMetadata", &value).map_err(WireError::from)?;
        if presented == &current {
            Ok(())
        } else {
            Err(WireError::new(ErrorCode::PreconditionFailed))
        }
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

    /// Commits the sealed generation under the request's replay identity.
    ///
    /// The receipt `Put` is a participant of the **same** transaction as the
    /// domain rows, so there is no window in which the effect landed and the
    /// record of it did not.
    async fn commit_sealed(
        &self,
        metadata: &StoredSecret,
        generation: &StoredGeneration,
        expected_revision: Option<SecretRevision>,
        quota: Option<Quota>,
        value: &models::SecretMetadata,
        now: Timestamp,
    ) -> WireResult<models::SecretMetadata> {
        // The route declares `IdempotencyKind::IdempotencyKey`, so the edge has
        // already refused a request without one. An absent identity here is an
        // edge defect, not a customer condition, and is reported as such rather
        // than quietly committing without a receipt.
        let identity = self.cx.idempotency.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InternalError)
                .with_message("this route requires a replay identity".to_owned())
        })?;
        let scope =
            IdempotencyScope::new("secret:set", Some(metadata.name.as_str())).map_err(|error| {
                WireError::new(ErrorCode::InvalidRequest).with_message(error.to_string())
            })?;
        let receipt = receipt_for(&scope, identity, SealedMetadata::KIND, value, now)?;

        let custody = self.shared.custody.as_ref();
        let receipts = CustodyReceipts(custody);
        let outcome = commit_or_replay::<SealedMetadata, _, _>(
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
                let plan = expressions::set(
                    &self.shared.custody_table,
                    generation,
                    metadata,
                    expected_revision,
                    Some(&receipt),
                    quota,
                )?;
                custody.commit(&plan).await?;
                Ok(SealedMetadata(value.clone()))
            },
        )
        .await
        .map_err(|error| write_failure(&error))?;
        Ok(outcome.into_inner().0)
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
        let scope = IdempotencyScope::new("secret:credential", Some(binding.provider.as_str()))
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
/// deployable — which is what makes it identical for both write paths.
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

impl SecretsApi for Routes {
    /// `DELETE /api/secrets/{name}` — the tombstone.
    ///
    /// The route declares no `not_found`, and that is deliberate: deleting a
    /// name that is absent, or already a tombstone, is a completed request. Both
    /// answer `204` **without a write**, so a retry is free and a repeated
    /// delete never advances the revision a concurrent editor is fencing on.
    async fn secret_delete(&self, _cx: &WireContext, name: ResourceName) -> WireResult<NoContent> {
        let workspace = self.cx.auth.workspace_id;
        let Some(stored) = self
            .shared
            .custody
            .load_secret(workspace, &name)
            .await
            .map_err(|error| authority_failure(&error))?
        else {
            return Ok(NoContent);
        };
        if stored.state == SecretState::Deleted {
            return Ok(NoContent);
        }
        self.check_if_match(&stored)?;
        let builder = expressions::delete(
            &self.shared.custody_table,
            workspace,
            name.as_str(),
            stored.revision,
            self.now()?,
        )
        .map_err(|error| authority_failure(&error))?;
        self.shared
            .custody
            .commit_update(builder, Participant::SECRET_METADATA)
            .await
            .map_err(|error| authority_failure(&error))?;
        Ok(NoContent)
    }

    async fn secret_get(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
    ) -> WireResult<WithETag<models::SecretMetadata>> {
        Err(not_served(RouteId::SecretGet))
    }

    /// `PUT /api/secrets/{name}` — the first seal.
    ///
    /// One read, one seal, one transaction, in that order and for a reason: the
    /// source generation is **inside** the encryption context
    /// (`CONTEXT_KEYS[5]`), so it has to be known before anything is sealed, and
    /// it is derived from the revision the read observed.
    ///
    /// Overwrite semantics (D-8):
    ///
    /// * `If-Match` is optional and, when present, is compared byte-for-byte
    ///   against the tag the read path would issue for the current stored
    ///   representation;
    /// * with or without it the write is always a compare-and-swap on the
    ///   observed revision, so two writers racing on one name leave one winner
    ///   and one `412` and lose nothing;
    /// * a `PUT` over a **revoked** name succeeds and is the intended rotation
    ///   path — the `O(1)` revoke raises `revokedThroughRevision` and leaves the
    ///   state alone, so the new revision reads `ready` again while every
    ///   session bound to an earlier revision stays fenced;
    /// * a `PUT` over a **tombstone** is refused: the route declares no
    ///   `not_found`, so it answers `precondition_failed`.
    async fn secret_put(
        &self,
        _cx: &WireContext,
        name: ResourceName,
        body: models::SecretPutRequest,
    ) -> WireResult<WithETag<models::SecretMetadata>> {
        // D-10. A credential's backing secret is reserved and invisible, and a
        // caller who guesses a `pcr_` id gets the same answer whether or not
        // that credential exists — which leaks nothing and needs no special
        // case.
        if is_reserved_name(&name) {
            return Err(WireError::new(ErrorCode::InvalidRequest).with_message(
                "this name is reserved for a provider credential; register one instead".to_owned(),
            ));
        }
        // The bytes leave the request body and enter `SecretPlaintext` in one
        // move, so from here the only owner is a type that cannot be serialized
        // and zeroizes on drop.
        let plaintext = projection::secret_plaintext(body).map_err(WireError::from)?;
        let workspace = self.cx.auth.workspace_id;
        let now = self.now()?;

        let stored = self
            .shared
            .custody
            .load_secret(workspace, &name)
            .await
            .map_err(|error| authority_failure(&error))?;
        if let Some(stored) = stored.as_ref() {
            if stored.state == SecretState::Deleted {
                return Err(WireError::new(ErrorCode::PreconditionFailed)
                    .with_message("this secret is deleted".to_owned()));
            }
            self.check_if_match(stored)?;
        } else if self.cx.if_match.is_some() {
            // A precondition against a representation that does not exist can
            // never be satisfied, and answering `200` would silently discard it.
            return Err(WireError::new(ErrorCode::PreconditionFailed)
                .with_message("this secret does not exist".to_owned()));
        }

        let (generation, revision, created_at, epoch) = match stored.as_ref() {
            Some(current) => (
                current.generation.next(),
                current.revision.next(),
                current.created_at,
                // A later `set` mints a new generation but never lowers the
                // epoch, so an existing session stays blocked until an explicit
                // rebind.
                current.revocation_epoch,
            ),
            None => (
                SourceGeneration::FIRST,
                SecretRevision::FIRST,
                now,
                RevocationEpoch::INITIAL,
            ),
        };

        let sealed = self
            .seal(workspace, &name, generation, &plaintext, now)
            .await?;
        let metadata = StoredSecret {
            workspace,
            name: name.clone(),
            generation,
            revision,
            state: SecretState::Ready,
            revocation_epoch: epoch,
            // The fence the observed row carried is preserved verbatim: a `set`
            // must never lower it, and the projection compares it against the
            // *new* revision, which has advanced past it.
            revoked_through_revision: stored.as_ref().map_or(SecretRevision(0), |current| {
                current.revoked_through_revision
            }),
            created_at,
            updated_at: now,
            revoked_at: None,
        };
        let expected_revision = stored.as_ref().map(|current| current.revision);
        // A replace does not consume quota: the collection does not grow, so a
        // client re-PUTting one name in a loop can never exhaust the bound.
        let quota = expected_revision
            .is_none()
            .then(|| Quota::secrets(self.shared.limits.max_secrets));

        let value = projection::secret_metadata(&metadata).map_err(WireError::from)?;
        let committed = self
            .commit_sealed(&metadata, &sealed, expected_revision, quota, &value, now)
            .await?;
        let etag = entity_tag(SealedMetadata::KIND, &committed).map_err(WireError::from)?;
        Ok(WithETag {
            value: committed,
            etag,
        })
    }

    /// `POST /api/secrets/{name}/revocations` — the emergency fence.
    ///
    /// The route declares an `Idempotency-Key`, and this handler needs no
    /// durable receipt to honour it. Revocation is terminal and the scope
    /// subject is the name, so one scope plus one key can only ever carry one
    /// intent: an already-fenced record is answered from its stored row and an
    /// `idempotency_conflict` is unreachable rather than undetected.
    async fn secret_revoke(
        &self,
        _cx: &WireContext,
        name: ResourceName,
        _body: models::EmptyRequest,
    ) -> WireResult<models::SecretRevocation> {
        let workspace = self.cx.auth.workspace_id;
        let stored = self
            .shared
            .custody
            .load_secret(workspace, &name)
            .await
            .map_err(|error| authority_failure(&error))?
            .filter(|stored| stored.state != SecretState::Deleted)
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;

        // Already fenced: the answer is the stored receipt, and no second update
        // is compiled. A second revoke would advance the epoch and move
        // `revokedAt`, which would make the same request answer differently.
        if stored.revoked_through_revision >= stored.revision {
            return projection::secret_revocation(&stored).map_err(WireError::from);
        }

        let now = self.now()?;
        let builder = expressions::revoke(
            &self.shared.custody_table,
            workspace,
            name.as_str(),
            stored.revocation_epoch,
            now,
        )
        .map_err(|error| authority_failure(&error))?;
        self.shared
            .custody
            .commit_update(builder, Participant::SECRET_METADATA)
            .await
            .map_err(|error| authority_failure(&error))?;

        // The committed row is exactly the observed one with the fence raised,
        // the instant stamped and the revision untouched, so the receipt is
        // built from what the transaction wrote rather than from a second read
        // that could observe a later state.
        let committed = StoredSecret {
            revoked_through_revision: stored.revision,
            revoked_at: Some(now),
            updated_at: now,
            ..stored
        };
        projection::secret_revocation(&committed).map_err(WireError::from)
    }

    async fn secrets_list(
        &self,
        _cx: &WireContext,
        _query: models::SecretsListQuery,
    ) -> WireResult<models::SecretMetadataPage> {
        Err(not_served(RouteId::SecretsList))
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

    /// `POST /api/secrets/provider-credentials` — register a BYOK key.
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
        let plaintext = SecretPlaintext::new(body.api_key.into_bytes())
            .map_err(|error| WireError::from(ProjectionError::from(error)))?;

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
        let outcome = match route(raw.route).fragment {
            "secrets" => dispatch_secrets(self, &wire, raw, limits).await?,
            "provider-credentials" => {
                dispatch_provider_credentials(self, &wire, raw, limits).await?
            }
            _ => return Err(not_served(raw.route)),
        };
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

/// The revision a first `set` writes, exposed so a composition test can build a
/// record without re-deriving the domain's constant.
pub const FIRST_REVISION: SecretRevision = SecretRevision::FIRST;
