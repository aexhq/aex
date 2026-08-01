//! The plaintext-boundary half of `regional:secrets`, over the real adapters.
//!
//! This deployable owns the four routes that carry or destroy key material. Two
//! of them — `secret_delete` and `secret_revoke` — need no cryptography at all:
//! a tombstone and a revocation fence are conditional updates on one metadata
//! row, and neither reads or writes a ciphertext. Those two are served. The two
//! that must seal a plaintext are not, and `references/rewrite/regional-services.md`
//! records exactly what they are waiting for.

use std::sync::Arc;

use aex_regional_http::context::RequestContext;
use aex_regional_http::mount::{UnaryDispatch, not_served};
use aex_regional_http::projection::{self, authority_failure, entity_tag};
use aex_regional_http::router::RouteOwner;
use aex_secret_custody_dynamodb::codec::SecretMetadata as StoredSecret;
use aex_secret_custody_dynamodb::expressions;
use aex_secret_custody_dynamodb::store::SecretCustodyStore;
use aex_secret_domain::secret::{SecretRevision, SecretState};
use aex_session_dynamodb::plan::Participant;
use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{ProviderCredentialId, ResourceName};
use aex_wire::models;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    AcceptKind, Created, NoContent, ProviderCredentialsApi, RequestContext as WireContext,
    SecretsApi, WithETag, dispatch_provider_credentials, dispatch_secrets,
};
use aex_wire::types::{ETag, Timestamp};

/// The adapters and start-up bindings every request shares.
pub struct Shared {
    /// The custody authority.
    pub custody: Arc<dyn SecretCustodyStore>,
    /// The physical `regional-secret-custody` table name.
    pub custody_table: String,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Shared")
            .field("custody_table", &self.custody_table)
            .finish_non_exhaustive()
    }
}

/// One request's view of the deployable.
pub struct Routes {
    shared: Arc<Shared>,
    cx: RequestContext,
}

/// The routes this deployable can answer completely today.
const SERVED: &[RouteId] = &[RouteId::SecretDelete, RouteId::SecretRevoke];

impl Routes {
    /// Binds the shared adapters to one verified request.
    #[must_use]
    pub const fn new(shared: Arc<Shared>, cx: RequestContext) -> Self {
        Self { shared, cx }
    }

    /// Every route whose handler is complete, in `RouteId` order.
    #[must_use]
    pub fn served() -> Vec<RouteId> {
        RouteOwner::SecretApi
            .routes()
            .into_iter()
            .filter(|id| SERVED.contains(id))
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
    /// `DELETE /api/workspace/secrets/{name}` — the tombstone.
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

    async fn secret_put(
        &self,
        _cx: &WireContext,
        _name: ResourceName,
        _body: models::SecretPutRequest,
    ) -> WireResult<WithETag<models::SecretMetadata>> {
        Err(not_served(RouteId::SecretPut))
    }

    /// `POST /api/workspace/secrets/{name}/revocations` — the emergency fence.
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

    async fn provider_credential_register(
        &self,
        _cx: &WireContext,
        _body: models::ProviderCredentialRegisterRequest,
    ) -> WireResult<Created<models::ProviderCredential>> {
        Err(not_served(RouteId::ProviderCredentialRegister))
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
