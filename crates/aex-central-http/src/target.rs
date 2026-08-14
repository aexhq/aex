//! Resolving the resource a request names, and the account state that gates it.
//!
//! `aex_control_domain::decide` needs a **fully resolved** resource: a workspace
//! carries its organization, a key carries its workspace and organization, an
//! operation carries its organization. Those come from the row, never from the
//! path, because a caller who could name a foreign organization in the path
//! could otherwise authorize itself against it.
//!
//! Resolution therefore happens at precedence stage 3, before the handler, and
//! it is a port so the edge stays testable without a database.

use std::sync::Arc;

use aex_control_app::ports::{ControlStore, StoreError};
use aex_control_domain::{
    AccountState, Action, Granted, Principal, Resource, ResourceClass, WorkspaceStatus, decide,
    requirement,
};
use aex_wire::ids::{ApiKeyId, OperationId, OrganizationId, PrefixedId, WorkspaceId};
use aex_wire::routes::{PathBinding, RouteId};
use async_trait::async_trait;
use uuid::Uuid;

use crate::error::{EdgeError, from_denial};

/// The bound path parameters, owned so they can cross an `async` boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetPath(Vec<(String, String)>);

impl TargetPath {
    /// Copies a matched binding.
    #[must_use]
    pub fn from_binding(binding: &PathBinding<'_>) -> Self {
        Self(
            binding
                .as_slice()
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    /// The value bound to `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(bound, _)| bound == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Where the resource a route names is read from.
#[async_trait]
pub trait TargetResolver: Send + Sync {
    /// The fully resolved resource `route` names, or `None` when it does not
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::AuthenticationUnavailable`] when the authority could
    /// not be reached; every other failure is the resolver's own.
    async fn resolve(
        &self,
        route: RouteId,
        path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError>;

    /// The **current** account state of an organization.
    ///
    /// There is deliberately no cached-positive path: an unreadable state
    /// rejects new admission, because serving a paused account and refusing a
    /// paying one are both worse than a retryable `503`.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::AccountStateUnavailable`] when the state could not
    /// be established.
    async fn account_state(&self, organization_id: Uuid) -> Result<AccountState, EdgeError>;
}

/// Runs precedence stages 3 to 6 for one request.
///
/// The order is fixed and matches the wire contract: resolve, decide, then gate
/// on the account state. A `403` is therefore always decided before a `402`, and
/// a caller learns nothing about an organization it may not act in.
///
/// # Errors
///
/// Returns the first stage that refused, as an [`EdgeError`].
pub async fn admit_request(
    resolver: &dyn TargetResolver,
    principal: &Principal,
    action: Action,
    path: &TargetPath,
) -> Result<Granted, EdgeError> {
    let requirement = requirement(action);
    let resource = if requirement.resource_class == ResourceClass::None {
        Resource::None
    } else {
        resolver
            .resolve(action.route(), path)
            .await?
            .ok_or(EdgeError::NotFound)?
    };

    // Decide first. Reading the account state of an organization the caller may
    // not act in would be a side effect it is not entitled to cause, and the
    // wire contract's precedence puts `403` before `402` for the same reason.
    let granted = decide(principal, action, &resource).map_err(from_denial)?;

    // A pause-exempt route never reads the state at all, so an unreadable
    // finance row cannot take down a route that does not depend on it.
    if requirement.pause_exempt {
        return Ok(granted);
    }
    let Some(organization_id) = granted.organization_id else {
        return Ok(granted);
    };
    match resolver.account_state(organization_id).await? {
        AccountState::Active => Ok(granted),
        AccountState::PausedTopUpRequired => Err(EdgeError::AccountPaused),
        AccountState::Unavailable => Err(EdgeError::AccountStateUnavailable),
    }
}

/// The resolver every control composition uses, over the coarse control port.
///
/// Each arm reads the **row** and takes the containing workspace and
/// organization from it. Nothing is taken from the path except the identifier
/// being looked up, which is why a caller who names a foreign organization in a
/// path cannot authorize itself against it: the decision runs against what the
/// row says the resource belongs to.
///
/// A deleted workspace is [`EdgeError::Gone`] rather than [`EdgeError::NotFound`].
/// The two are different instructions — one says "you had the wrong name", the
/// other says "this existed and does not any more" — and only the second tells
/// a client to stop retrying.
pub struct ControlStoreTargets {
    store: Arc<dyn ControlStore>,
}

impl std::fmt::Debug for ControlStoreTargets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ControlStoreTargets").finish()
    }
}

impl ControlStoreTargets {
    /// Builds the resolver over one control authority.
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self { store }
    }

    /// The identifier a path binds under `name`, in its wire form.
    ///
    /// An identifier that does not parse names nothing, so it is
    /// [`EdgeError::NotFound`] rather than a `400`: telling an unauthorized
    /// caller apart "that is not an identifier" from "that identifier is not
    /// yours" is information it has not earned, and the route's own declared
    /// codes do not include a validation failure on a path parameter.
    fn bound<T: PrefixedId>(path: &TargetPath, name: &'static str) -> Result<Uuid, EdgeError> {
        let text = path.get(name).ok_or(EdgeError::Internal(
            "a route bound no path parameter its resource class requires",
        ))?;
        let parsed = T::parse(text).map_err(|_| EdgeError::NotFound)?;
        Ok(Uuid::from_bytes(*parsed.uuid7().as_bytes()))
    }

    /// A store failure at resolution time is never a `404`.
    ///
    /// `NotFound` from the store means the read ran and matched nothing, which
    /// the caller of this function turns into `404` itself. Everything else is
    /// an authority that did not answer, and answering `404` for that would
    /// tell a caller its resource is gone because a database was busy.
    const fn unreadable(error: &StoreError) -> EdgeError {
        match error {
            StoreError::NotFound => EdgeError::NotFound,
            _ => EdgeError::AuthenticationUnavailable,
        }
    }
}

#[async_trait]
impl TargetResolver for ControlStoreTargets {
    async fn resolve(
        &self,
        route: RouteId,
        path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError> {
        let action = Action::central(route).map_err(|_| {
            EdgeError::Internal("a regional route reached the central target resolver")
        })?;
        match requirement(action).resource_class {
            // `admit_request` never calls this arm; it short-circuits an
            // actor-scoped route before the resolver is consulted. Answering
            // `None` here would render as `404` for a route that names nothing.
            ResourceClass::None => Ok(Some(Resource::None)),
            ResourceClass::Organization => {
                let id = Self::bound::<OrganizationId>(path, "organizationId")?;
                let found = self
                    .store
                    .get_organization(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|organization| Resource::Organization(organization.id)))
            }
            ResourceClass::Workspace => {
                let id = Self::bound::<WorkspaceId>(path, "workspaceId")?;
                let Some(workspace) = self
                    .store
                    .get_workspace(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?
                else {
                    return Ok(None);
                };
                if workspace.status == WorkspaceStatus::Deleted {
                    return Err(EdgeError::Gone);
                }
                Ok(Some(Resource::Workspace {
                    workspace_id: workspace.id,
                    organization_id: workspace.organization_id,
                }))
            }
            ResourceClass::ApiKey => {
                let id = Self::bound::<ApiKeyId>(path, "apiKeyId")?;
                let found = self
                    .store
                    .get_api_key(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|key| Resource::ApiKey {
                    key_id: key.id,
                    workspace_id: key.workspace_id,
                    organization_id: key.organization_id,
                }))
            }
            ResourceClass::Operation => {
                let id = Self::bound::<OperationId>(path, "operationId")?;
                let found = self
                    .store
                    .get_operation(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|operation| Resource::Operation {
                    operation_id: operation.id,
                    organization_id: operation.organization_id,
                }))
            }
        }
    }

    async fn account_state(&self, organization_id: Uuid) -> Result<AccountState, EdgeError> {
        // A failure is never softened into a state. The port already answers
        // `Unavailable` for an organization whose finance row is absent, so the
        // only thing left to map here is a store that did not answer at all —
        // and that is the same refusal, not a fallback to `Active`.
        self.store
            .account_state(organization_id)
            .await
            .map_err(|_| EdgeError::AccountStateUnavailable)
    }
}
