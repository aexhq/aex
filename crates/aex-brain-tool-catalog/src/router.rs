//! Composed application `ToolPort` over frozen catalog routes and executors.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, RedactedDetail,
    ToolDispatchError, ToolOutcome, ToolPort, ToolRoute, ToolRoutingError,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{
    CatalogPin, ContentHash, DetachedOperationId, Fence, ToolName as DomainToolName,
};
use aex_brain_domain::journal::ExecutorRoute as DomainExecutorRoute;

use crate::manifest::{EntryState, ToolManifestEntry};
use crate::wire_pending::ExecutorRoute;

/// One executor linked into the mux composition.
pub trait ToolExecutor: Send + Sync + 'static {
    /// Invokes one already-routed call.
    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>>;

    /// Queries exactly one committed durable operation id.
    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>>;

    /// Best-effort fenced cancellation.
    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>>;
}

#[derive(Debug, Clone)]
struct InstalledRoute {
    route: ToolRoute,
    admitted: bool,
}

/// Immutable catalog routes plus exactly one executor per coarse Brain route.
/// Runtime dispatch never discovers or re-routes a tool.
#[derive(Default)]
pub struct CompositeToolRouter {
    catalogs: BTreeMap<CatalogPin, BTreeMap<DomainToolName, InstalledRoute>>,
    executors: Vec<(DomainExecutorRoute, Arc<dyn ToolExecutor>)>,
    operations: Mutex<BTreeMap<DetachedOperationId, DomainExecutorRoute>>,
}

impl CompositeToolRouter {
    /// Empty composition.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the sole ready executor for one route.
    ///
    /// # Errors
    ///
    /// Duplicate route claims fail closed.
    pub fn register_executor(
        &mut self,
        route: DomainExecutorRoute,
        executor: Arc<dyn ToolExecutor>,
    ) -> Result<(), RouterBuildError> {
        if self
            .executors
            .iter()
            .any(|(registered, _)| *registered == route)
        {
            return Err(RouterBuildError::DuplicateExecutor { route });
        }
        self.executors.push((route, executor));
        Ok(())
    }

    /// Installs one frozen catalog pin.
    ///
    /// # Errors
    ///
    /// Refuses a pin/digest mismatch, duplicate pin, or duplicate tool name.
    pub fn install_catalog(
        &mut self,
        pin: CatalogPin,
        digest: ContentHash,
        entries: &[ToolManifestEntry],
    ) -> Result<(), RouterBuildError> {
        if pin.0 != digest {
            return Err(RouterBuildError::PinDigestMismatch);
        }
        if self.catalogs.contains_key(&pin) {
            return Err(RouterBuildError::DuplicateCatalog);
        }
        let mut routes = BTreeMap::new();
        for entry in entries {
            let name = DomainToolName(entry.descriptor.name.as_str().to_owned());
            let route = ToolRoute {
                name: name.clone(),
                executor: coarse_route(entry.descriptor.route),
                class: entry.descriptor.effect,
                timeout_ms: entry.descriptor.bounds.timeout_ms,
                manifest_digest: digest,
            };
            if routes
                .insert(
                    name.clone(),
                    InstalledRoute {
                        route,
                        admitted: matches!(entry.state, EntryState::Active),
                    },
                )
                .is_some()
            {
                return Err(RouterBuildError::DuplicateTool { name: name.0 });
            }
        }
        self.catalogs.insert(pin, routes);
        Ok(())
    }

    fn executor(
        &self,
        route: DomainExecutorRoute,
    ) -> Result<&Arc<dyn ToolExecutor>, ToolDispatchError> {
        self.executors
            .iter()
            .find(|(registered, _)| *registered == route)
            .map(|(_, executor)| executor)
            .ok_or_else(|| ToolDispatchError {
                stage: DispatchStage::PreDispatch,
                proof: DispatchProof::NotSent,
                retryable: false,
                detail: RedactedDetail::new("tool executor is not ready"),
            })
    }
}

impl ToolPort for CompositeToolRouter {
    fn route(
        &self,
        pin: &CatalogPin,
        name: &DomainToolName,
    ) -> Result<ToolRoute, ToolRoutingError> {
        let catalog = self
            .catalogs
            .get(pin)
            .ok_or(ToolRoutingError::UnknownPin { pin: pin.0 })?;
        let installed = catalog.get(name).ok_or_else(|| ToolRoutingError::Unknown {
            name: name.0.clone(),
        })?;
        if !installed.admitted {
            return Err(ToolRoutingError::NotAdmitted {
                name: name.0.clone(),
            });
        }
        Ok(installed.route.clone())
    }

    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ToolDispatchError {
                    stage: DispatchStage::PreDispatch,
                    proof: DispatchProof::NotSent,
                    retryable: false,
                    detail: RedactedDetail::new("tool call cancelled before dispatch"),
                });
            }
            let executor = self.executor(call.route.executor)?;
            let outcome = executor.invoke(ticket, call, cancel).await?;
            if let ToolOutcome::Detached { operation, .. } = &outcome {
                self.operations
                    .lock()
                    .expect("tool operation mutex")
                    .insert(operation.clone(), call.route.executor);
            }
            Ok(outcome)
        })
    }

    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            let route = self
                .operations
                .lock()
                .expect("tool operation mutex")
                .get(operation)
                .copied();
            let Some(route) = route else {
                return Ok(DetachedStatus::Unknown);
            };
            self.executor(route)?.query(operation).await
        })
    }

    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async move {
            let route = self
                .operations
                .lock()
                .expect("tool operation mutex")
                .get(operation)
                .copied();
            let Some(route) = route else {
                return Ok(());
            };
            self.executor(route)?.cancel(operation, fence).await
        })
    }
}

const fn coarse_route(route: ExecutorRoute) -> DomainExecutorRoute {
    match route {
        ExecutorRoute::Control | ExecutorRoute::Park | ExecutorRoute::SubagentScheduler => {
            DomainExecutorRoute::BrainInline
        }
        ExecutorRoute::ManagedWeb => DomainExecutorRoute::ManagedWeb,
        ExecutorRoute::Mcp => DomainExecutorRoute::Mcp,
        ExecutorRoute::HandsFilesystem
        | ExecutorRoute::HandsDevelopment
        | ExecutorRoute::HandsBrowser
        | ExecutorRoute::RegisteredCustom => DomainExecutorRoute::Hands,
    }
}

/// Composition-time route failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouterBuildError {
    /// Two executors claimed one route.
    #[error("duplicate executor for {route:?}")]
    DuplicateExecutor {
        /// Ambiguous route.
        route: DomainExecutorRoute,
    },
    /// Catalog pin did not equal its supplied digest.
    #[error("catalog pin does not match digest")]
    PinDigestMismatch,
    /// Same immutable pin was installed twice.
    #[error("catalog pin installed twice")]
    DuplicateCatalog,
    /// Catalog contained the same name twice.
    #[error("catalog contains duplicate tool `{name}`")]
    DuplicateTool {
        /// Duplicate name.
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use aex_brain_application::ports::{ToolPort as _, ToolRoutingError};
    use aex_brain_domain::ids::{CatalogPin, ContentHash, ToolName};

    use super::CompositeToolRouter;
    use crate::catalog::builtin_entries;

    #[test]
    fn routing_is_exactly_pinned_and_staged_entries_fail_closed() {
        let digest = ContentHash([7; 32]);
        let pin = CatalogPin(digest);
        let mut router = CompositeToolRouter::new();
        let entries = builtin_entries().expect("built-in fixture");
        router
            .install_catalog(pin, digest, &entries)
            .expect("catalog install");
        let name = ToolName("web_fetch".to_owned());
        let route = router.route(&pin, &name).expect("active route");
        assert_eq!(route.name, name);
        assert_eq!(route.manifest_digest, digest);
        assert!(matches!(
            router.route(
                &CatalogPin(ContentHash([8; 32])),
                &ToolName("web_fetch".into())
            ),
            Err(ToolRoutingError::UnknownPin { .. })
        ));
    }
}
