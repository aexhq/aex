use std::sync::Arc;

use aex_runtime_control::HandId;

use crate::contract::{
    ExecutorOutput, FullOutput, MAX_LIVE_PREVIEW_BYTES, ReadyHand, SandboxConfig, ToolCompletion,
    ToolError, ToolHandle, ToolHandleRequest, ToolRead, ToolStart, ToolStartRequest, ToolTarget,
};
use crate::ports::{GuestPort, McpPort, ResultRetentionPort, RuntimePort, StoragePersistPort};
use crate::telemetry::{TelemetryEvent, TelemetryKind, TelemetryPort, TelemetryProducer};

/// Application service composing routing, readiness, execution and result retention.
pub struct ToolMux {
    runtime: Arc<dyn RuntimePort>,
    guest: Arc<dyn GuestPort>,
    mcp: Arc<dyn McpPort>,
    storage: Arc<dyn StoragePersistPort>,
    results: Arc<dyn ResultRetentionPort>,
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
        mcp: Arc<dyn McpPort>,
        storage: Arc<dyn StoragePersistPort>,
        results: Arc<dyn ResultRetentionPort>,
        telemetry: Arc<dyn TelemetryPort>,
    ) -> Self {
        Self {
            runtime,
            guest,
            mcp,
            storage,
            results,
            telemetry: TelemetryProducer::new(telemetry),
        }
    }

    /// Starts eager setup after session admission. Disabled sessions make no
    /// Runtime Control call.
    ///
    /// # Errors
    ///
    /// Returns a redacted runtime failure. Telemetry is fail-open.
    pub async fn eager_prepare(
        &self,
        session: aex_wire::ids::SessionId,
        sandbox: SandboxConfig,
    ) -> Result<(), String> {
        if !sandbox.enabled {
            return Ok(());
        }
        let generation = sandbox
            .generation
            .ok_or_else(|| "enabled sandbox has no exact generation".to_owned())?;
        let hand = HandId::for_session(session);
        self.emit(TelemetryEvent {
            session,
            hand: Some(hand),
            generation: Some(generation),
            call: None,
            kind: TelemetryKind::SandboxRequested,
        });
        for progress in self.runtime.eager_prepare(session, sandbox).await? {
            self.emit(TelemetryEvent {
                session,
                hand: Some(hand),
                generation: Some(generation),
                call: None,
                kind: TelemetryKind::SandboxProgress { progress },
            });
        }
        Ok(())
    }

    /// Starts one call. Remote MCP bypasses the Hand; every other target waits
    /// for the session-derived Hand and exact generation.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport/control failure. Tool failures are normal
    /// [`ToolCompletion`] values.
    pub async fn start(&self, request: &ToolStartRequest) -> Result<ToolStart, String> {
        if matches!(request.target, ToolTarget::RemoteMcp { .. }) {
            return self.start_remote_mcp(request).await;
        }
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
            TelemetryKind::ToolWaiting,
        );
        let ready = self
            .runtime
            .wait_ready(
                request.identity.session,
                hand,
                generation,
                &request.identity,
            )
            .await?;
        if ready.hand != hand || ready.generation != generation {
            return Err("Runtime Control returned a foreign Hand generation".to_owned());
        }
        let outcome = self.start_ready(request, ready).await;
        if matches!(outcome, Ok(ToolStart::Accepted { .. })) {
            return outcome;
        }
        let settled = self.runtime.settle_waiter(ready, &request.identity).await;
        match (outcome, settled) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    async fn start_ready(
        &self,
        request: &ToolStartRequest,
        ready: ReadyHand,
    ) -> Result<ToolStart, String> {
        self.guest.hello(ready).await?;
        self.emit_for(
            request,
            Some(ready.hand),
            Some(ready.generation),
            TelemetryKind::ToolStarted,
        );

        let started = match &request.target {
            ToolTarget::OfficialSandbox { .. } | ToolTarget::SandboxMcp { .. } => self
                .guest
                .start(
                    ready,
                    &request.target,
                    &request.arguments,
                    &request.identity,
                )
                .await?
                .map_err(|operation| ToolHandle::Sandbox {
                    hand: ready.hand,
                    generation: ready.generation,
                    fence: ready.fence,
                    operation,
                    max_result_bytes: request.max_result_bytes,
                    timeout_ms: request.timeout_ms,
                }),
            ToolTarget::StoragePersist {
                source,
                logical_name,
                media_type,
            } => self
                .storage
                .persist(
                    ready,
                    source,
                    logical_name,
                    media_type.as_deref(),
                    &request.identity,
                )
                .await
                .map(Ok)?,
            ToolTarget::RemoteMcp { .. } => unreachable!("remote MCP returned above"),
        };

        match started {
            Ok(output) => Ok(ToolStart::Completed {
                result: self.finish_output(request, Some(ready), output).await?,
            }),
            Err(handle) => Ok(ToolStart::Accepted { handle }),
        }
    }

    /// Reads a detached result without creating another executor operation.
    ///
    /// # Errors
    ///
    /// Refuses foreign exact-generation handles and redacted executor failures.
    pub async fn read(&self, request: &ToolHandleRequest) -> Result<ToolRead, String> {
        let output = match &request.handle {
            ToolHandle::Sandbox {
                hand,
                generation,
                fence,
                operation,
                max_result_bytes,
                timeout_ms,
            } => {
                if *hand != HandId::for_session(request.identity.session) {
                    return Err("tool handle belongs to a foreign Hand".to_owned());
                }
                let ready = ReadyHand {
                    hand: *hand,
                    generation: *generation,
                    fence: *fence,
                };
                self.guest.hello(ready).await?;
                let Some(output) = self
                    .guest
                    .read(ready, *operation, *max_result_bytes, *timeout_ms)
                    .await?
                else {
                    return Ok(ToolRead::Pending);
                };
                let result = self
                    .finish_detached(&request.identity, Some(ready), output)
                    .await?;
                self.runtime.settle_waiter(ready, &request.identity).await?;
                return Ok(ToolRead::Completed { result });
            }
            ToolHandle::RemoteMcp { .. } | ToolHandle::StoragePersist { .. } => {
                self.results.read_handle(&request.handle).await?
            }
        };
        let Some(output) = output else {
            return Ok(ToolRead::Pending);
        };
        Ok(ToolRead::Completed {
            result: self
                .finish_detached(&request.identity, None, output)
                .await?,
        })
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
                fence,
                operation,
                ..
            } => {
                if *hand != HandId::for_session(request.identity.session) {
                    return Err("tool handle belongs to a foreign Hand".to_owned());
                }
                let outcome = self
                    .guest
                    .cancel(
                        ReadyHand {
                            hand: *hand,
                            generation: *generation,
                            fence: *fence,
                        },
                        *operation,
                    )
                    .await;
                let settled = self
                    .runtime
                    .settle_waiter(
                        ReadyHand {
                            hand: *hand,
                            generation: *generation,
                            fence: *fence,
                        },
                        &request.identity,
                    )
                    .await;
                match (outcome, settled) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
            ToolHandle::RemoteMcp { .. } | ToolHandle::StoragePersist { .. } => Ok(()),
        }
    }

    async fn start_remote_mcp(&self, request: &ToolStartRequest) -> Result<ToolStart, String> {
        let ToolTarget::RemoteMcp {
            server,
            endpoint,
            headers,
            tool,
        } = &request.target
        else {
            unreachable!("the caller matched remote MCP")
        };
        self.emit_for(request, None, None, TelemetryKind::RemoteMcpStarted);
        let output = self
            .mcp
            .call_remote(
                endpoint,
                headers,
                server,
                tool,
                &request.arguments,
                &request.identity,
            )
            .await?;
        let result = self.finish_output(request, None, output).await?;
        Ok(ToolStart::Completed { result })
    }

    async fn finish_output(
        &self,
        request: &ToolStartRequest,
        ready: Option<ReadyHand>,
        output: ExecutorOutput,
    ) -> Result<ToolCompletion, String> {
        self.finish_detached(&request.identity, ready, output).await
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
        let retained = match &output.full {
            FullOutput::Inline(body) => Some(self.results.retain_inline(identity, body).await?),
            FullOutput::SandboxFile { path, bytes, hash } => {
                let ready = ready.ok_or_else(|| {
                    "a remote executor claimed a sandbox-backed full result".to_owned()
                })?;
                Some(
                    self.results
                        .retain_sandbox_file(identity, ready, path, *bytes, *hash)
                        .await?,
                )
            }
        };
        let complete_bytes = retained.as_ref().map_or(0, |result| result.bytes);
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
        if let Some(retained) = &retained {
            self.emit_identity(
                identity,
                ready.map(|value| value.hand),
                ready.map(|value| value.generation),
                TelemetryKind::ToolResultRetained {
                    object_ref: retained.object_ref.clone(),
                    bytes: retained.bytes,
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
            retained,
            error: output.is_error.then(|| ToolError {
                code: "tool_error".to_owned(),
                message:
                    "The tool returned an error result; inspect the preview or retained output."
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
