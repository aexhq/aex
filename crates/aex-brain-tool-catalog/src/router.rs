//! Composed application `ToolPort` over frozen catalog routes and executors.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolPort, ToolRoute, ToolRoutingError,
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

    /// Associates one verified tool manifest with a frozen model-catalog pin.
    ///
    /// `pin` and `digest` deliberately use different algorithms and identify
    /// different artifacts. The composition that verified both makes the
    /// association explicitly; this router never compares their raw bytes.
    ///
    /// # Errors
    ///
    /// Refuses a duplicate tool name or a name outside the shared grammar.
    pub fn install_catalog(
        &mut self,
        pin: CatalogPin,
        digest: ContentHash,
        entries: &[ToolManifestEntry],
    ) -> Result<(), RouterBuildError> {
        let mut candidates = BTreeMap::new();
        for entry in entries {
            let name = DomainToolName::parse(entry.descriptor.name.as_str())
                .map_err(|_| RouterBuildError::InvalidToolName)?;
            let route = ToolRoute {
                name: name.clone(),
                executor: coarse_route(entry.descriptor.route),
                class: entry.descriptor.effect,
                timeout_ms: entry.descriptor.bounds.timeout_ms,
                manifest_digest: digest,
            };
            if candidates
                .insert(
                    name.clone(),
                    InstalledRoute {
                        route,
                        admitted: matches!(entry.state, EntryState::Active),
                    },
                )
                .is_some()
            {
                return Err(RouterBuildError::DuplicateTool {
                    name: name.as_str().to_owned(),
                });
            }
        }
        if let Some(routes) = self.catalogs.get(&pin)
            && let Some(name) = candidates.keys().find(|name| routes.contains_key(*name))
        {
            return Err(RouterBuildError::DuplicateTool {
                name: name.as_str().to_owned(),
            });
        }
        self.catalogs.entry(pin).or_default().extend(candidates);
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
                detail: RedactedDetail::internal(
                    ProviderFailureKind::ServerError,
                    "tool executor is not ready",
                ),
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
            .ok_or(ToolRoutingError::UnknownPin { pin: *pin })?;
        let installed = catalog.get(name).ok_or_else(|| ToolRoutingError::Unknown {
            name: name.as_str().to_owned(),
        })?;
        if !installed.admitted {
            return Err(ToolRoutingError::NotAdmitted {
                name: name.as_str().to_owned(),
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
                    detail: RedactedDetail::internal(
                        ProviderFailureKind::Cancelled,
                        "tool call cancelled before dispatch",
                    ),
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
    /// Catalog contained the same name twice.
    #[error("catalog contains duplicate tool `{name}`")]
    DuplicateTool {
        /// Duplicate name.
        name: String,
    },
    /// A signed manifest carried a name outside the shared resource-name grammar.
    #[error("catalog contains a tool name outside the shared resource-name grammar")]
    InvalidToolName,
}

#[cfg(test)]
mod tests {
    use aex_brain_application::ports::{ToolPort as _, ToolRoutingError};
    use aex_brain_domain::ids::{CatalogPin, ContentHash, ToolName};
    use aex_model_catalog::Blake3Digest;

    use super::CompositeToolRouter;
    use crate::catalog::builtin_entries;

    #[test]
    fn routing_is_exactly_pinned_and_staged_entries_fail_closed() {
        let digest = ContentHash([7; 32]);
        let pin = CatalogPin(Blake3Digest::of(b"model catalog"));
        let mut router = CompositeToolRouter::new();
        let entries = builtin_entries().expect("built-in fixture");
        router
            .install_catalog(pin, digest, &entries)
            .expect("catalog install");
        let name = ToolName::parse("web_fetch").expect("tool name");
        let route = router.route(&pin, &name).expect("active route");
        assert_eq!(route.name, name);
        assert_eq!(route.manifest_digest, digest);
        assert!(matches!(
            router.route(
                &CatalogPin(Blake3Digest::from_bytes([8; 32])),
                &ToolName::parse("web_fetch").expect("tool name")
            ),
            Err(ToolRoutingError::UnknownPin { .. })
        ));
    }
}
