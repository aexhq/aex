//! Brain's production Hands port adapter.
//!
//! The lower backend owns generation materialization and guest RPC. This adapter owns the
//! trust boundary back into Brain: an endpoint, acceptance, or result naming a successor
//! generation is rejected before it can enter the journal.

use std::sync::Arc;

use aex_brain_app::ports::{
    BoxFuture, DispatchTicket, HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart,
    HandsOperationStatus, HandsPort, HandsResult, ResultBounds,
};
use aex_brain_domain::ids::{Fence, HandsOperationId, SessionId};
use aex_wire::ids::GenerationId;

/// Materialization and guest-RPC backend consumed by [`HandsAdapter`].
///
/// Implementations are responsible for the runtime-activity conditional writes, provider
/// lifecycle calls, authenticated guest transport, and protocol frame bounds. The adapter
/// deliberately does not expose an untyped address-only shortcut: every operation still
/// names the exact canonical generation.
pub trait HandsBackend: Send + Sync + 'static {
    /// Ensures the named generation is reachable.
    fn ensure_generation<'a>(
        &'a self,
        session: &'a SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>>;

    /// Starts or finds one deterministic operation.
    fn start<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        generation: GenerationId,
        start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>>;

    /// Queries one operation without creating another.
    fn status<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>>;

    /// Best-effort fenced cancellation.
    fn cancel<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>>;

    /// Reads a terminal result under the caller's bounds.
    fn result<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
        bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>>;
}

/// Exact-generation enforcing implementation of Brain's [`HandsPort`].
#[derive(Clone)]
pub struct HandsAdapter {
    backend: Arc<dyn HandsBackend>,
}

impl HandsAdapter {
    /// Binds the trusted adapter to one runtime backend.
    #[must_use]
    pub fn new(backend: Arc<dyn HandsBackend>) -> Self {
        Self { backend }
    }
}

impl core::fmt::Debug for HandsAdapter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HandsAdapter")
            .finish_non_exhaustive()
    }
}

impl HandsPort for HandsAdapter {
    fn ensure_generation<'a>(
        &'a self,
        session: &'a SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
        Box::pin(async move {
            let endpoint = self.backend.ensure_generation(session, generation).await?;
            require_generation(generation, endpoint.generation)?;
            Ok(endpoint)
        })
    }

    fn start<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        generation: GenerationId,
        start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
        Box::pin(async move {
            let accepted = self.backend.start(ticket, generation, start).await?;
            require_generation(generation, accepted.generation)?;
            if accepted.operation != start.operation {
                return Err(HandsError::OperationMismatch {
                    expected: start.operation.clone(),
                    found: accepted.operation,
                });
            }
            Ok(accepted)
        })
    }

    fn status<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
        self.backend.status(generation, operation)
    }

    fn cancel<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        self.backend.cancel(generation, operation, fence)
    }

    fn result<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a HandsOperationId,
        bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
        Box::pin(async move {
            let result = self.backend.result(generation, operation, bounds).await?;
            require_generation(generation, result.generation)?;
            if result.operation != *operation {
                return Err(HandsError::OperationMismatch {
                    expected: operation.clone(),
                    found: result.operation,
                });
            }
            Ok(result)
        })
    }
}

fn require_generation(expected: GenerationId, found: GenerationId) -> Result<(), HandsError> {
    if expected == found {
        Ok(())
    } else {
        Err(HandsError::GenerationMismatch { expected, found })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{HandsAdapter, HandsBackend};
    use aex_brain_app::ports::{
        BoxFuture, CancelToken, DispatchTicket, FenceGuard, HandsAccepted, HandsEndpoint,
        HandsError, HandsOperationStart, HandsOperationStatus, HandsPort as _, HandsResult,
        ResultBounds,
    };
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence,
        HandsOperationId, OwnerToken, SessionId, Timestamp,
    };
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7, WorkspaceId};

    fn generation(seed: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn session() -> SessionId {
        SessionId(uuid::Uuid::from_u128(1))
    }

    fn operation() -> HandsOperationId {
        HandsOperationId("operation-1".to_owned())
    }

    fn ticket() -> DispatchTicket {
        let guard = FenceGuard::new(
            AgentKey::new(session(), AgentId(uuid::Uuid::from_u128(2))),
            OwnerToken(uuid::Uuid::from_u128(3)),
            Fence(4),
            AgentRevision(5),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10])),
            aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [9; 10])),
            EffectId([7; 16]),
            1,
            Timestamp::from_millis(0),
        )
    }

    #[derive(Debug)]
    struct RecordingBackend {
        returned_generation: GenerationId,
        returned_operation: Option<HandsOperationId>,
        calls: Mutex<Vec<GenerationId>>,
    }

    impl HandsBackend for RecordingBackend {
        fn ensure_generation<'a>(
            &'a self,
            _session: &'a SessionId,
            generation: GenerationId,
        ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(generation);
                Ok(HandsEndpoint {
                    generation: self.returned_generation,
                    address: "guest.internal".to_owned(),
                    lease_expires_at: Timestamp::from_millis(60_000),
                })
            })
        }

        fn start<'a>(
            &'a self,
            _ticket: &'a DispatchTicket,
            generation: GenerationId,
            start: &'a HandsOperationStart,
        ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(generation);
                Ok(HandsAccepted {
                    operation: self
                        .returned_operation
                        .clone()
                        .unwrap_or_else(|| start.operation.clone()),
                    generation: self.returned_generation,
                    created: true,
                    poll_after: core::time::Duration::from_millis(250),
                    result: None,
                })
            })
        }

        fn status<'a>(
            &'a self,
            generation: GenerationId,
            _operation: &'a HandsOperationId,
        ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(generation);
                Ok(HandsOperationStatus::Running {
                    poll_after: core::time::Duration::from_millis(250),
                })
            })
        }

        fn cancel<'a>(
            &'a self,
            generation: GenerationId,
            _operation: &'a HandsOperationId,
            _fence: Fence,
        ) -> BoxFuture<'a, Result<(), HandsError>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(generation);
                Ok(())
            })
        }

        fn result<'a>(
            &'a self,
            generation: GenerationId,
            operation: &'a HandsOperationId,
            _bounds: &'a ResultBounds,
        ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(generation);
                Ok(HandsResult {
                    operation: self
                        .returned_operation
                        .clone()
                        .unwrap_or_else(|| operation.clone()),
                    generation: self.returned_generation,
                    exit_code: 0,
                    inline: Some("ok".to_owned()),
                    placed: None,
                    truncated: false,
                    duration_ms: 1,
                    checksum: ContentHash::of(b"ok"),
                })
            })
        }
    }

    fn adapter(returned_generation: GenerationId) -> (HandsAdapter, Arc<RecordingBackend>) {
        let backend = Arc::new(RecordingBackend {
            returned_generation,
            returned_operation: None,
            calls: Mutex::new(Vec::new()),
        });
        (HandsAdapter::new(backend.clone()), backend)
    }

    fn start() -> HandsOperationStart {
        HandsOperationStart {
            operation: operation(),
            call_hash: ContentHash::of(b"call"),
            request: serde_json::json!({"command":"true"}),
            bounds: ResultBounds {
                max_bytes: 1_024,
                max_stream_bytes: 512,
                timeout_ms: 30_000,
            },
            deadline: Timestamp::from_millis(60_000),
        }
    }

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("fixture unexpectedly parked"),
        }
    }

    #[test]
    fn every_call_forwards_the_exact_canonical_generation() {
        let expected = generation(1);
        let (adapter, backend) = adapter(expected);
        block_on(adapter.ensure_generation(&session(), expected)).expect("endpoint");
        block_on(adapter.start(&ticket(), expected, &start())).expect("accepted");
        block_on(adapter.status(expected, &operation())).expect("status");
        block_on(adapter.cancel(expected, &operation(), Fence(4))).expect("cancel");
        block_on(adapter.result(
            expected,
            &operation(),
            &ResultBounds {
                max_bytes: 1_024,
                max_stream_bytes: 512,
                timeout_ms: 30_000,
            },
        ))
        .expect("result");
        assert_eq!(
            backend.calls.lock().expect("calls").as_slice(),
            &[expected; 5]
        );
    }

    #[test]
    fn a_successor_generation_can_never_impersonate_the_requested_one() {
        let expected = generation(1);
        let successor = generation(2);
        let (adapter, _) = adapter(successor);
        assert!(matches!(
            block_on(adapter.ensure_generation(&session(), expected)),
            Err(HandsError::GenerationMismatch {
                expected: seen,
                found
            }) if seen == expected && found == successor
        ));
        assert!(matches!(
            block_on(adapter.start(&ticket(), expected, &start())),
            Err(HandsError::GenerationMismatch { .. })
        ));
        assert!(matches!(
            block_on(adapter.result(
                expected,
                &operation(),
                &ResultBounds {
                    max_bytes: 1_024,
                    max_stream_bytes: 512,
                    timeout_ms: 30_000,
                },
            )),
            Err(HandsError::GenerationMismatch { .. })
        ));
    }

    #[test]
    fn a_backend_response_for_another_operation_is_rejected_explicitly() {
        let expected_generation = generation(1);
        let expected_operation = operation();
        let found_operation = HandsOperationId("operation-2".to_owned());
        let backend = Arc::new(RecordingBackend {
            returned_generation: expected_generation,
            returned_operation: Some(found_operation.clone()),
            calls: Mutex::new(Vec::new()),
        });
        let adapter = HandsAdapter::new(backend);

        assert!(matches!(
            block_on(adapter.start(&ticket(), expected_generation, &start())),
            Err(HandsError::OperationMismatch { expected, found })
                if expected == expected_operation && found == found_operation
        ));
        assert!(matches!(
            block_on(adapter.result(
                expected_generation,
                &expected_operation,
                &ResultBounds {
                    max_bytes: 1_024,
                    max_stream_bytes: 512,
                    timeout_ms: 30_000,
                },
            )),
            Err(HandsError::OperationMismatch { expected, found })
                if expected == expected_operation && found == found_operation
        ));
    }
}
