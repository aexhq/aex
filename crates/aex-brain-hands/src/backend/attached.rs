//! Brain-side attached delivery selection, client call, and result incorporation.

use super::{
    AttachResponse, AuthenticatedGuestEndpoint, BrainContentHash, DeliveryMode, DispatchProof,
    Duration, GenerationId, HandsAccepted, HandsError, HandsOperationId, HandsOperationStart,
    HandsResult, LeaseHandle, MAX_RESULT_BODY_BYTES, ProductionHandsBackend, ProviderFailureKind,
    ResultChunk, StartRequest, TerminalMetadata, TerminalState, Verb, exit_code, poll_after,
    require_operation, response_error, result_rejected,
};

/// Which delivery mode one declared wall bound selects.
#[must_use]
pub const fn delivery_for(_timeout_ms: u32) -> DeliveryMode {
    DeliveryMode::Detached
}

impl ProductionHandsBackend {
    /// Holds one connection until the attached terminal answer arrives.
    #[expect(
        clippy::too_many_arguments,
        reason = "attached delivery binds the authenticated endpoint, exact operation, request, bounds and timeout"
    )]
    pub(super) async fn start_attached(
        &self,
        endpoint: &LeaseHandle<AuthenticatedGuestEndpoint>,
        native_resume: Option<&super::GenerationView>,
        generation: GenerationId,
        operation: HandsOperationId,
        start: &HandsOperationStart,
        call: &StartRequest,
        timeout: Duration,
    ) -> Result<HandsAccepted, HandsError> {
        let reply = match self
            .call_guest::<_, AttachResponse>(endpoint, Verb::Attach, call, timeout)
            .await
        {
            Ok(reply) => reply,
            Err(
                error @ HandsError::Transport {
                    proof: DispatchProof::NotSent,
                    ..
                },
            ) => {
                self.settle_operation(generation, operation).await?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if let Some(view) = native_resume {
            self.settle_native_activity(view).await?;
        }
        match reply {
            AttachResponse::Terminal {
                operation: found,
                existing,
                terminal,
                chunk,
                ..
            } => {
                require_operation(operation, found)?;
                let result = self
                    .incorporate_attached(generation, operation, start, &terminal, chunk.as_ref())
                    .await?;
                Ok(HandsAccepted {
                    operation: start.operation.clone(),
                    generation,
                    created: !existing,
                    poll_after: Duration::ZERO,
                    result,
                })
            }
            AttachResponse::NotTerminal {
                operation: found, ..
            } => {
                require_operation(operation, found)?;
                Ok(HandsAccepted {
                    operation: start.operation.clone(),
                    generation,
                    created: false,
                    poll_after: poll_after(0),
                    result: None,
                })
            }
            AttachResponse::Conflict {
                operation: found, ..
            } => {
                require_operation(operation, found)?;
                self.settle_operation(generation, operation).await?;
                Err(HandsError::CallHashConflict {
                    operation: start.operation.clone(),
                })
            }
            AttachResponse::Rejected {
                operation: found, ..
            } => {
                require_operation(operation, found)?;
                self.settle_operation(generation, operation).await?;
                Err(response_error(
                    ProviderFailureKind::InvalidRequest,
                    "the guest rejected the Hands operation",
                ))
            }
        }
    }

    /// Verifies and settles an attached terminal, or declines it to the pull.
    async fn incorporate_attached(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
        start: &HandsOperationStart,
        terminal: &TerminalMetadata,
        chunk: Option<&ResultChunk>,
    ) -> Result<Option<Box<HandsResult>>, HandsError> {
        let maximum = u64::try_from(start.bounds.max_bytes)
            .unwrap_or(u64::MAX)
            .min(MAX_RESULT_BODY_BYTES);
        if terminal.body_len > maximum
            || matches!(
                terminal.state,
                TerminalState::Cancelled | TerminalState::Interrupted
            )
        {
            return Ok(None);
        }
        let bytes = match chunk {
            None if terminal.body_len == 0 => Vec::new(),
            Some(chunk) if chunk.last && chunk.offset == 0 => {
                require_operation(operation, chunk.operation)?;
                chunk.bytes.clone()
            }
            _ => return Ok(None),
        };
        let bytes = super::ResultAssembly::resume(operation, terminal.clone(), bytes, maximum)
            .finish()
            .map_err(|_| {
                result_rejected(
                    &start.operation,
                    "the attached terminal result failed length or digest verification",
                )
            })?;
        self.settle_operation(generation, operation).await?;
        let (inline, sandbox_file) = super::result_body(&start.operation, terminal, bytes)?;
        let duration_ms = terminal
            .ended_at
            .unix_millis()
            .saturating_sub(terminal.started_at.unix_millis());
        Ok(Some(Box::new(HandsResult {
            operation: start.operation.clone(),
            generation,
            exit_code: exit_code(&terminal.exit),
            inline,
            sandbox_file,
            truncated: terminal.truncated,
            duration_ms: u32::try_from(duration_ms.max(0)).unwrap_or(u32::MAX),
            checksum: BrainContentHash(*terminal.digest.as_bytes()),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::delivery_for;
    use aex_brain_tool_catalog::wire_pending::ExecutorRoute as CatalogRoute;
    use aex_hands_protocol::operation::DeliveryMode;

    #[test]
    fn every_operation_starts_detached_regardless_of_its_wall_bound() {
        for timeout_ms in [
            1,
            15_000,
            30_000,
            60_000,
            60_001,
            120_000,
            600_000,
            1_800_000,
            u32::MAX,
        ] {
            assert_eq!(
                delivery_for(timeout_ms),
                DeliveryMode::Detached,
                "{timeout_ms} ms"
            );
        }
    }

    #[test]
    fn the_mvp_catalogue_starts_every_sandbox_tool_detached() {
        let entries =
            aex_brain_tool_catalog::catalog::builtin_entries().expect("the catalogue builds");
        let mut attached = Vec::new();
        let mut detached = Vec::new();
        for entry in &entries {
            if !matches!(
                entry.descriptor.route,
                CatalogRoute::HandsFilesystem
                    | CatalogRoute::HandsDevelopment
                    | CatalogRoute::HandsBrowser
            ) {
                continue;
            }
            let name = entry.descriptor.name.as_str().to_owned();
            match delivery_for(entry.descriptor.bounds.timeout_ms) {
                DeliveryMode::Attached => attached.push(name),
                DeliveryMode::Detached => detached.push(name),
            }
        }
        attached.sort();
        detached.sort();
        assert_eq!(
            detached,
            vec!["bash", "edit_file", "read_file", "write_file"]
        );
        assert!(attached.is_empty());
    }
}
