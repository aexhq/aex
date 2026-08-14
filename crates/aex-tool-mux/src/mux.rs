use std::sync::Arc;

use aex_runtime_control::HandId;

use crate::contract::{
    ExecutorOutput, FullOutput, MAX_LIVE_PREVIEW_BYTES, ReadyHand, SandboxResultFile,
    ToolCompletion, ToolError, ToolHandle, ToolHandleRequest, ToolRead, ToolStart,
    ToolStartRequest, ToolTarget,
};
use crate::ports::{GuestPort, RuntimePort, StoragePersistPort};
use crate::telemetry::{TelemetryEvent, TelemetryKind, TelemetryPort, TelemetryProducer};

/// Application service composing routing, readiness, detached execution and local results.
pub struct ToolMux {
    runtime: Arc<dyn RuntimePort>,
    guest: Arc<dyn GuestPort>,
    storage: Arc<dyn StoragePersistPort>,
    telemetry: TelemetryProducer,
}

impl core::fmt::Debug for ToolMux {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("ToolMux").finish_non_exhaustive()
    }
}

impl ToolMux {
    /// Composes the only trusted tool broker.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimePort>,
        guest: Arc<dyn GuestPort>,
        storage: Arc<dyn StoragePersistPort>,
        telemetry: Arc<dyn TelemetryPort>,
    ) -> Self {
        Self::with_telemetry_producer(runtime, guest, storage, TelemetryProducer::new(telemetry))
    }

    /// Composes the broker with a producer shared by detached preparation.
    #[must_use]
    pub fn with_telemetry_producer(
        runtime: Arc<dyn RuntimePort>,
        guest: Arc<dyn GuestPort>,
        storage: Arc<dyn StoragePersistPort>,
        telemetry: TelemetryProducer,
    ) -> Self {
        Self {
            runtime,
            guest,
            storage,
            telemetry,
        }
    }

    /// Starts one call behind its durable preparation waiter and returns promptly.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport/control failure. Tool failures are normal
    /// [`ToolCompletion`] values.
    pub async fn start(&self, request: &ToolStartRequest) -> Result<ToolStart, String> {
        if !request.sandbox.enabled {
            return Ok(ToolStart::Completed {
                result: ToolCompletion::sandbox_disabled(),
            });
        }
        let generation = request
            .sandbox
            .generation
            .ok_or_else(|| "enabled sandbox has no exact generation".to_owned())?;
        let hand = HandId::for_session(request.identity.session);
        self.emit_for(
            request,
            Some(hand),
            Some(generation),
            TelemetryKind::SandboxRequested,
        );
        self.emit_for(
            request,
            Some(hand),
            Some(generation),
            TelemetryKind::ToolWaiting,
        );
        self.runtime
            .start_waiter(
                request.identity.session,
                hand,
                generation,
                &request.identity,
            )
            .await?;
        Ok(ToolStart::Accepted {
            handle: ToolHandle::Sandbox {
                hand,
                generation,
                target: Box::new(request.target.clone()),
                arguments: request.arguments.clone(),
                deadline_ms: request.deadline_ms,
                max_result_bytes: request.max_result_bytes,
                timeout_ms: request.timeout_ms,
            },
        })
    }

    /// Reads a detached result without creating another executor operation.
    ///
    /// # Errors
    ///
    /// Refuses foreign exact-generation handles and redacted executor failures.
    pub async fn read(&self, request: &ToolHandleRequest) -> Result<ToolRead, String> {
        let (ready, output) = match &request.handle {
            ToolHandle::Sandbox {
                hand,
                generation,
                target,
                arguments,
                deadline_ms,
                max_result_bytes,
                timeout_ms,
            } => {
                if *hand != HandId::for_session(request.identity.session) {
                    return Err("tool handle belongs to a foreign Hand".to_owned());
                }
                let Some(ready) = self
                    .runtime
                    .poll_waiter(
                        request.identity.session,
                        *hand,
                        *generation,
                        &request.identity,
                    )
                    .await?
                else {
                    return Ok(ToolRead::Pending);
                };
                if ready.hand != *hand || ready.generation != *generation {
                    return Err("Runtime Control returned a foreign Hand generation".to_owned());
                }
                let output = async {
                    self.guest.hello(ready).await?;
                    self.emit_identity(
                        &request.identity,
                        Some(ready.hand),
                        Some(ready.generation),
                        TelemetryKind::ToolStarted,
                    );
                    match target.as_ref() {
                        ToolTarget::OfficialSandbox { .. }
                        | ToolTarget::RemoteMcp { .. }
                        | ToolTarget::SandboxMcp { .. } => {
                            if matches!(target.as_ref(), ToolTarget::RemoteMcp { .. }) {
                                self.emit_identity(
                                    &request.identity,
                                    Some(ready.hand),
                                    Some(ready.generation),
                                    TelemetryKind::RemoteMcpStarted,
                                );
                            }
                            let operation = self
                                .guest
                                .start(
                                    ready,
                                    target,
                                    arguments,
                                    &request.identity,
                                    *deadline_ms,
                                    *max_result_bytes,
                                    *timeout_ms,
                                )
                                .await?;
                            self.guest
                                .read(ready, operation, *max_result_bytes, *timeout_ms)
                                .await
                        }
                        ToolTarget::StoragePersist {
                            source,
                            logical_name,
                            media_type,
                        } => {
                            self.storage
                                .start_persist(
                                    ready,
                                    source,
                                    logical_name,
                                    media_type.as_deref(),
                                    &request.identity,
                                )
                                .await?;
                            self.storage.read_persist(ready, &request.identity).await
                        }
                    }
                }
                .await;
                let output = match output {
                    Ok(output) => output,
                    Err(error) => {
                        let _ = self.runtime.settle_waiter(ready, &request.identity).await;
                        return Err(error);
                    }
                };
                (ready, output)
            }
        };
        let Some(output) = output else {
            return Ok(ToolRead::Pending);
        };
        let result = self
            .finish_detached(&request.identity, Some(ready), output)
            .await?;
        self.runtime.settle_waiter(ready, &request.identity).await?;
        Ok(ToolRead::Completed { result })
    }

    /// Best-effort exact-handle cancellation. It never starts another attempt.
    ///
    /// # Errors
    ///
    /// Returns a redacted executor failure.
    pub async fn cancel(&self, request: &ToolHandleRequest) -> Result<(), String> {
        match &request.handle {
            ToolHandle::Sandbox {
                hand,
                generation,
                target,
                ..
            } => {
                if *hand != HandId::for_session(request.identity.session) {
                    return Err("tool handle belongs to a foreign Hand".to_owned());
                }
                let Some(ready) = self
                    .runtime
                    .cancel_waiter(*generation, &request.identity)
                    .await?
                else {
                    return Ok(());
                };
                if ready.hand != *hand || ready.generation != *generation {
                    return Err("cancel recovery returned a foreign generation".to_owned());
                }
                let outcome = match target.as_ref() {
                    ToolTarget::OfficialSandbox { .. }
                    | ToolTarget::RemoteMcp { .. }
                    | ToolTarget::SandboxMcp { .. } => {
                        self.guest.cancel(ready, &request.identity).await
                    }
                    ToolTarget::StoragePersist { .. } => {
                        self.storage.cancel_persist(ready, &request.identity).await
                    }
                };
                let settled = self.runtime.settle_waiter(ready, &request.identity).await;
                match (outcome, settled) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
        }
    }

    async fn finish_detached(
        &self,
        identity: &crate::ToolCallIdentity,
        ready: Option<ReadyHand>,
        output: ExecutorOutput,
    ) -> Result<ToolCompletion, String> {
        let preview_len = output.preview.len().min(MAX_LIVE_PREVIEW_BYTES);
        let preview = String::from_utf8_lossy(&output.preview[..preview_len]).into_owned();
        let preview_was_cut = output.preview.len() > preview_len;
        let (complete_bytes, output_file) = match &output.full {
            FullOutput::Inline(body) => (body.len() as u64, None),
            FullOutput::SandboxFile { path, bytes, hash } => (
                *bytes,
                Some(SandboxResultFile {
                    path: path.as_str().to_owned(),
                    bytes: *bytes,
                    hash: *hash,
                }),
            ),
        };
        let truncated = preview_was_cut || complete_bytes > preview_len as u64;
        self.emit_identity(
            identity,
            ready.map(|value| value.hand),
            ready.map(|value| value.generation),
            TelemetryKind::ToolPreview {
                bytes: preview_len as u64,
                truncated,
            },
        );
        if let Some(output_file) = &output_file {
            self.emit_identity(
                identity,
                ready.map(|value| value.hand),
                ready.map(|value| value.generation),
                TelemetryKind::ToolResultPlaced {
                    path: output_file.path.clone(),
                    bytes: output_file.bytes,
                },
            );
        }
        self.emit_identity(
            identity,
            ready.map(|value| value.hand),
            ready.map(|value| value.generation),
            TelemetryKind::ToolCompleted {
                is_error: output.is_error,
            },
        );
        Ok(ToolCompletion {
            preview,
            truncated,
            output_file,
            error: output.is_error.then(|| ToolError {
                code: "tool_error".to_owned(),
                message:
                    "The tool returned an error result; inspect the preview or local output file."
                        .to_owned(),
            }),
        })
    }

    fn emit_for(
        &self,
        request: &ToolStartRequest,
        hand: Option<HandId>,
        generation: Option<aex_wire::ids::GenerationId>,
        kind: TelemetryKind,
    ) {
        self.emit_identity(&request.identity, hand, generation, kind);
    }

    fn emit_identity(
        &self,
        identity: &crate::ToolCallIdentity,
        hand: Option<HandId>,
        generation: Option<aex_wire::ids::GenerationId>,
        kind: TelemetryKind,
    ) {
        self.emit(TelemetryEvent {
            session: identity.session,
            hand,
            generation,
            call: Some(identity.call.clone()),
            kind,
        });
    }

    fn emit(&self, event: TelemetryEvent) {
        self.telemetry.emit(event);
    }
}
