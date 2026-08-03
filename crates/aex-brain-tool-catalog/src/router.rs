//! Composed application `ToolPort` over frozen catalog routes and executors.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolPort, ToolRoute, ToolRoutingError,
};
use aex_brain_domain::effect::DetachedOperationRef;
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
pub struct CompositeToolRouter {
    catalogs: BTreeMap<CatalogPin, BTreeMap<DomainToolName, InstalledRoute>>,
    executors: [Option<Arc<dyn ToolExecutor>>; 4],
}

impl Default for CompositeToolRouter {
    fn default() -> Self {
        Self {
            catalogs: BTreeMap::new(),
            executors: std::array::from_fn(|_| None),
        }
    }
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
        let slot = &mut self.executors[executor_slot(route)];
        if slot.is_some() {
            return Err(RouterBuildError::DuplicateExecutor { route });
        }
        *slot = Some(executor);
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
        self.executors[executor_slot(route)]
            .as_ref()
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
            if let ToolOutcome::Completed(body) = &outcome
                && body.executed_on != call.route.executor
            {
                return Err(executor_mismatch(call.route.executor, body.executed_on));
            }
            Ok(outcome)
        })
    }

    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            let status = self
                .executor(operation.executor)?
                .query(&operation.id)
                .await?;
            if let DetachedStatus::Completed(body) = &status
                && body.executed_on != operation.executor
            {
                return Err(executor_mismatch(operation.executor, body.executed_on));
            }
            Ok(status)
        })
    }

    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationRef,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async move {
            self.executor(operation.executor)?
                .cancel(&operation.id, fence)
                .await
        })
    }
}

const fn executor_slot(route: DomainExecutorRoute) -> usize {
    match route {
        DomainExecutorRoute::BrainInline => 0,
        DomainExecutorRoute::ManagedWeb => 1,
        DomainExecutorRoute::Mcp => 2,
        DomainExecutorRoute::Hands => 3,
    }
}

fn executor_mismatch(
    _expected: DomainExecutorRoute,
    _found: DomainExecutorRoute,
) -> ToolDispatchError {
    ToolDispatchError {
        stage: DispatchStage::Terminal,
        proof: DispatchProof::ResponseStarted,
        retryable: false,
        detail: RedactedDetail::internal(
            ProviderFailureKind::ProtocolViolation,
            "tool executor receipt route does not match the durable route",
        ),
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
    use std::sync::{Arc, Mutex};

    use aex_brain_application::ports::{
        BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall,
        ToolDispatchError, ToolOutcome, ToolPort as _, ToolResultBody, ToolRoutingError,
    };
    use aex_brain_domain::effect::DetachedOperationRef;
    use aex_brain_domain::ids::{CatalogPin, ContentHash, DetachedOperationId, Fence, ToolName};
    use aex_brain_domain::journal::ExecutorRoute;
    use aex_model_catalog::Blake3Digest;

    use super::{CompositeToolRouter, ToolExecutor};
    use crate::catalog::builtin_entries;

    #[derive(Debug)]
    struct RecordingExecutor {
        route: ExecutorRoute,
        queried: Mutex<Vec<DetachedOperationId>>,
        cancelled: Mutex<Vec<(DetachedOperationId, Fence)>>,
        mismatched_receipt: bool,
    }

    impl RecordingExecutor {
        fn new(route: ExecutorRoute) -> Self {
            Self {
                route,
                queried: Mutex::new(Vec::new()),
                cancelled: Mutex::new(Vec::new()),
                mismatched_receipt: false,
            }
        }

        fn with_mismatched_receipt(route: ExecutorRoute) -> Self {
            Self {
                mismatched_receipt: true,
                ..Self::new(route)
            }
        }
    }

    impl ToolExecutor for RecordingExecutor {
        fn invoke<'a>(
            &'a self,
            _ticket: &'a DispatchTicket,
            _call: &'a PreparedToolCall,
            _cancel: &'a CancelToken,
        ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
            unreachable!("the routing recovery tests never dispatch new work")
        }

        fn query<'a>(
            &'a self,
            operation: &'a DetachedOperationId,
        ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
            Box::pin(async move {
                self.queried
                    .lock()
                    .expect("not poisoned")
                    .push(operation.clone());
                let executed_on = if self.mismatched_receipt {
                    ExecutorRoute::BrainInline
                } else {
                    self.route
                };
                Ok(DetachedStatus::Completed(Box::new(ToolResultBody {
                    content: Vec::new(),
                    is_error: false,
                    duration_ms: 1,
                    executed_on,
                    checksum: ContentHash::of(b"result"),
                })))
            })
        }

        fn cancel<'a>(
            &'a self,
            operation: &'a DetachedOperationId,
            fence: Fence,
        ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
            Box::pin(async move {
                self.cancelled
                    .lock()
                    .expect("not poisoned")
                    .push((operation.clone(), fence));
                Ok(())
            })
        }
    }

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("a router fixture must complete immediately"),
        }
    }

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

    #[test]
    fn a_fresh_router_queries_and_cancels_directly_from_the_durable_ref() {
        let executor = Arc::new(RecordingExecutor::new(ExecutorRoute::Mcp));
        let mut restarted = CompositeToolRouter::new();
        restarted
            .register_executor(ExecutorRoute::Mcp, executor.clone())
            .expect("one exact executor");
        let operation = DetachedOperationRef {
            id: DetachedOperationId("operation-after-restart".to_owned()),
            executor: ExecutorRoute::Mcp,
        };

        assert!(matches!(
            block_on(restarted.query(&operation)),
            Ok(DetachedStatus::Completed(_))
        ));
        block_on(restarted.cancel(&operation, Fence(9))).expect("exact cancellation");
        assert_eq!(
            *executor.queried.lock().expect("not poisoned"),
            vec![operation.id.clone()]
        );
        assert_eq!(
            *executor.cancelled.lock().expect("not poisoned"),
            vec![(operation.id, Fence(9))]
        );
    }

    #[test]
    fn equal_raw_ids_on_different_executors_never_collide_or_broadcast() {
        let web = Arc::new(RecordingExecutor::new(ExecutorRoute::ManagedWeb));
        let mcp = Arc::new(RecordingExecutor::new(ExecutorRoute::Mcp));
        let mut router = CompositeToolRouter::new();
        router
            .register_executor(ExecutorRoute::ManagedWeb, web.clone())
            .expect("web executor");
        router
            .register_executor(ExecutorRoute::Mcp, mcp.clone())
            .expect("mcp executor");
        let id = DetachedOperationId("same-raw-id".to_owned());

        block_on(router.query(&DetachedOperationRef {
            id: id.clone(),
            executor: ExecutorRoute::ManagedWeb,
        }))
        .expect("web query");
        block_on(router.query(&DetachedOperationRef {
            id: id.clone(),
            executor: ExecutorRoute::Mcp,
        }))
        .expect("mcp query");

        assert_eq!(*web.queried.lock().expect("not poisoned"), vec![id.clone()]);
        assert_eq!(*mcp.queried.lock().expect("not poisoned"), vec![id]);
    }

    #[test]
    fn missing_or_mismatched_executor_routes_fail_closed() {
        let empty = CompositeToolRouter::new();
        let operation = DetachedOperationRef {
            id: DetachedOperationId("op".to_owned()),
            executor: ExecutorRoute::Hands,
        };
        assert!(block_on(empty.query(&operation)).is_err());
        assert!(block_on(empty.cancel(&operation, Fence(1))).is_err());

        let mut router = CompositeToolRouter::new();
        router
            .register_executor(
                ExecutorRoute::Mcp,
                Arc::new(RecordingExecutor::with_mismatched_receipt(
                    ExecutorRoute::Mcp,
                )),
            )
            .expect("mcp executor");
        assert!(
            block_on(router.query(&DetachedOperationRef {
                id: DetachedOperationId("op".to_owned()),
                executor: ExecutorRoute::Mcp,
            }))
            .is_err(),
            "an executor may not attribute its result to another route"
        );
    }
}
