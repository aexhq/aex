//! Bounded direct-provider router over the six admitted BYOK dialects.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DispatchTicket, EffectStore, PreviewSink, ProviderDispatchError,
    ProviderOutcome, ProviderPort, StreamBudget as PortStreamBudget, UnknownResolution,
};
use aex_brain_domain::effect::{DispatchEvidence, DispatchProof, DispatchStage, DurableEffect};
use aex_model_catalog::canonical::{
    CorrelationId, CredentialBindingRef, PreviewFrame, ProviderReceipt,
    RateLimitSource as ReceiptRateLimitSource, ReceiptBounds, ReceiptRateLimit, seal,
};
use aex_model_catalog::document::{AdapterSourceDigest, EntryState};
use aex_model_catalog::{BoundedString, ProviderFailureKind, RedactedDetail};
use aex_wire::provider::ProviderId;
use futures::StreamExt as _;

use crate::adapter::{BoundedBody, FrameOutcome, HeaderView, ProviderAdapter, RequestBuildError};
use crate::anthropic::AnthropicAdapter;
use crate::budget::{BudgetOverrun, StreamBudget};
use crate::credential::{
    BindingState, CredentialCache, CredentialResolveError, ProviderCredentialDecryptor,
    ProviderCredentialDirectory, SessionCredentialPin,
};
use crate::deepseek::DeepSeekAdapter;
use crate::error::{ProviderFailure, RateLimitFeedback, RateLimitSource};
use crate::google::GoogleAdapter;
use crate::moonshotai::MoonshotAdapter;
use crate::openai::OpenAiAdapter;
use crate::pool::{ClientPool, IsolationKey, PoolError};
use crate::sse::{SseDecoder, SseError};
use crate::transport::{ExecuteError, SendState, execute};
use crate::zai::ZaiAdapter;

/// A direct-provider router with no gateway, arbitrary endpoint or fallback.
pub struct ProviderRouter {
    directory: Arc<dyn ProviderCredentialDirectory>,
    decryptor: Arc<dyn ProviderCredentialDecryptor>,
    effects: Arc<dyn EffectStore>,
    cache: CredentialCache,
    pool: ClientPool,
    adapter_source: AdapterSourceDigest,
}

impl core::fmt::Debug for ProviderRouter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProviderRouter")
            .field("cache", &self.cache)
            .field("pool", &self.pool)
            .field("adapter_source", &self.adapter_source)
            .finish_non_exhaustive()
    }
}

impl ProviderRouter {
    /// Composes the bounded shared core around credential and effect authorities,
    /// using only the immutable source-tree identity stamped into this build.
    ///
    /// # Errors
    ///
    /// Returns [`crate::build_identity::AdapterBuildIdentityError`] when the
    /// binary was not produced by the release builder with a valid adapter stamp.
    pub fn from_build(
        directory: Arc<dyn ProviderCredentialDirectory>,
        decryptor: Arc<dyn ProviderCredentialDecryptor>,
        effects: Arc<dyn EffectStore>,
    ) -> Result<Self, crate::build_identity::AdapterBuildIdentityError> {
        Ok(Self {
            directory,
            decryptor,
            effects,
            cache: CredentialCache::default(),
            pool: ClientPool::default(),
            adapter_source: crate::build_identity::adapter_source_digest()?,
        })
    }

    /// Refuses new pool acquisitions and drops warm clients.
    pub fn drain(&self) {
        self.pool.drain();
        self.pool.close(&|_| true);
    }

    fn invalidate_binding(
        &self,
        ticket: &DispatchTicket,
        binding: &crate::credential::ProviderCredentialBinding,
    ) {
        let _ = self.cache.invalidate(ticket.workspace(), binding.id);
        self.pool.close(&|key| {
            key.organization == ticket.organization()
                && key.workspace == ticket.workspace()
                && key.credential_binding == binding.id
                && key.credential_generation == binding.generation
        });
    }

    /// Dispatches with the immutable credential binding a session admitted.
    ///
    /// # Errors
    ///
    /// Returns a typed provider dispatch failure with an exact send proof.
    #[allow(
        clippy::too_many_lines,
        reason = "the linear send-proof state machine is kept together so every await and retry visibly preserves its dispatch proof"
    )]
    pub(crate) async fn dispatch_pinned(
        &self,
        ticket: &DispatchTicket,
        pin: SessionCredentialPin,
        request: &aex_model_catalog::canonical::CanonicalModelRequest,
        port_budget: &PortStreamBudget,
        preview: &dyn PreviewSink,
        cancel: &CancelToken,
    ) -> Result<ProviderOutcome, ProviderDispatchError> {
        validate_request(ticket, request, self.adapter_source)?;
        let budget = gateway_budget(ticket, port_budget, request)?;
        let started = wire_timestamp(ticket.issued_at())?;
        let started_steady = Instant::now();
        if cancel.is_cancelled() {
            return Err(error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::Cancelled,
                "dispatch was cancelled before send",
            ));
        }

        let provider = request.selection.provider();
        let binding = await_pre_send(
            self.directory.resolve(
                ticket.organization(),
                ticket.workspace(),
                provider,
                Some(pin.binding),
            ),
            &budget,
            started_steady,
            cancel,
        )
        .await?;
        validate_binding(ticket, provider, pin, &binding)?;
        let key = await_pre_send(
            self.cache
                .decrypt(&binding, self.decryptor.as_ref(), started),
            &budget,
            started_steady,
            cancel,
        )
        .await?;
        let pooled = self
            .pool
            .acquire(
                &IsolationKey {
                    origin: request.selection.endpoint(),
                    organization: ticket.organization(),
                    workspace: ticket.workspace(),
                    credential_binding: binding.id,
                    credential_revision: binding.revision,
                    credential_generation: binding.generation,
                    catalog: request.selection.catalog(),
                },
                &budget,
            )
            .map_err(pool_error)?;
        let _permit = pooled
            .inflight
            .clone()
            .try_acquire_owned()
            .map_err(|_| pool_error(PoolError::PermitUnavailable))?;
        let adapter = adapter(provider);
        let wire = adapter
            .build_request(&request.selection, request)
            .map_err(build_error)?;
        let request_bytes = wire.body.len() as u64;

        let mut attempt = 0_u16;
        loop {
            attempt = attempt.saturating_add(1);
            let prior_proof = if attempt == 1 {
                DispatchProof::NotSent
            } else {
                DispatchProof::ResponseStarted
            };
            if cancel.is_cancelled() {
                return Err(error(
                    if attempt == 1 {
                        DispatchStage::PreDispatch
                    } else {
                        DispatchStage::Terminal
                    },
                    if attempt == 1 {
                        DispatchProof::NotSent
                    } else {
                        // A prior in-call attempt received a definitive 429/503
                        // response. The next send did not happen, but claiming
                        // the whole effect was never sent would erase that fact.
                        DispatchProof::ResponseStarted
                    },
                    ProviderFailureKind::Cancelled,
                    "dispatch was cancelled before send",
                ));
            }
            // This is deliberately the last authority I/O before the external
            // send, and it runs again for every retry. No database transaction
            // can be atomic with provider I/O; this narrows the unavoidable
            // TOCTOU window to local request assembly and socket submission.
            let current_epoch = match await_send_fence(
                self.directory.revalidate(&binding),
                &budget,
                started_steady,
                cancel,
                prior_proof,
            )
            .await
            {
                Ok(epoch) => epoch,
                Err(failure) => {
                    self.invalidate_binding(ticket, &binding);
                    return Err(failure);
                }
            };
            if let Err(failure) = validate_current_epoch(pin, &binding, current_epoch) {
                self.invalidate_binding(ticket, &binding);
                return Err(credential_error_with_proof(failure, prior_proof));
            }
            let mut send = SendState::new();
            let response = tokio::time::timeout(
                budget
                    .head_timeout
                    .min(remaining(&budget, started_steady, send.proof())?),
                execute(&pooled.http, &wire, &key, &mut send),
            )
            .await
            .map_err(|_| {
                budget_error(
                    BudgetOverrun::Head {
                        after: budget.head_timeout,
                    },
                    send.proof(),
                )
            })?
            .map_err(execute_error)?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            if !(200..300).contains(&status) {
                let body = read_error_body(response, &budget, cancel, &key, started_steady).await;
                let failure = adapter.classify_http(status, &HeaderView::new(&headers), &body);
                let policy = &request.selection.entry().retry_policy;
                if attempt < policy.max_attempts && policy.permits(status) {
                    let delay = Duration::from_millis(u64::from(policy.backoff_ms(attempt, 500)));
                    if delay
                        >= remaining(&budget, started_steady, DispatchProof::ResponseStarted)
                            .unwrap_or(Duration::ZERO)
                    {
                        return Err(provider_failure(
                            DispatchStage::Terminal,
                            DispatchProof::ResponseStarted,
                            failure,
                        ));
                    }
                    cancellable_sleep(delay, cancel).await?;
                    continue;
                }
                return Err(provider_failure(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    failure,
                ));
            }
            return self
                .consume_success(
                    ticket,
                    request,
                    adapter,
                    response,
                    headers,
                    status,
                    request_bytes,
                    &budget,
                    preview,
                    cancel,
                    started,
                    started_steady,
                    attempt,
                    &binding,
                )
                .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::too_many_lines,
        reason = "the streaming state machine is kept linear so byte, frame, idle, cancellation, and durable proof bounds can be audited in order"
    )]
    async fn consume_success(
        &self,
        ticket: &DispatchTicket,
        request: &aex_model_catalog::canonical::CanonicalModelRequest,
        adapter: &dyn ProviderAdapter,
        response: reqwest::Response,
        headers: reqwest::header::HeaderMap,
        http_status: u16,
        request_bytes: u64,
        budget: &StreamBudget,
        preview: &dyn PreviewSink,
        cancel: &CancelToken,
        started: aex_wire::types::Timestamp,
        started_steady: Instant,
        attempts: u16,
        binding: &crate::credential::ProviderCredentialBinding,
    ) -> Result<ProviderOutcome, ProviderDispatchError> {
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::new(budget.max_frame_bytes);
        let mut state = adapter.new_state(&request.selection);
        let mut response_bytes = 0_u64;
        let mut first_frame_at = None;
        let mut response_started = false;
        let mut terminal = false;

        while !terminal {
            if cancel.is_cancelled() {
                return Err(error(
                    DispatchStage::Streaming,
                    if response_started {
                        DispatchProof::ResponseStarted
                    } else {
                        DispatchProof::PossiblySent
                    },
                    ProviderFailureKind::Cancelled,
                    "dispatch was cancelled while streaming",
                ));
            }
            let timeout = if response_started {
                budget.idle_frame_timeout
            } else {
                budget.first_frame_timeout
            }
            .min(remaining(
                budget,
                started_steady,
                if response_started {
                    DispatchProof::ResponseStarted
                } else {
                    DispatchProof::PossiblySent
                },
            )?);
            let chunk = tokio::time::timeout(timeout, stream.next())
                .await
                .map_err(|_| {
                    let overrun = if response_started {
                        BudgetOverrun::IdleFrame { after: timeout }
                    } else {
                        BudgetOverrun::FirstFrame { after: timeout }
                    };
                    budget_error(
                        overrun,
                        if response_started {
                            DispatchProof::ResponseStarted
                        } else {
                            DispatchProof::PossiblySent
                        },
                    )
                })?;
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.map_err(|_| {
                error(
                    DispatchStage::Streaming,
                    if response_started {
                        DispatchProof::ResponseStarted
                    } else {
                        DispatchProof::PossiblySent
                    },
                    ProviderFailureKind::Transport,
                    "provider stream transport failed",
                )
            })?;
            response_bytes = response_bytes.saturating_add(chunk.len() as u64);
            if response_bytes > budget.max_response_bytes {
                return Err(budget_error(
                    BudgetOverrun::Response {
                        limit: budget.max_response_bytes,
                    },
                    if response_started {
                        DispatchProof::ResponseStarted
                    } else {
                        DispatchProof::PossiblySent
                    },
                ));
            }
            decoder
                .push(&chunk)
                .map_err(|failure| sse_error(failure, response_started))?;
            for event in decoder
                .drain()
                .map_err(|failure| sse_error(failure, response_started))?
            {
                let outcome = adapter
                    .decode(&mut state, &event.as_ref(), budget)
                    .map_err(|_| {
                        error(
                            DispatchStage::Streaming,
                            if response_started {
                                DispatchProof::ResponseStarted
                            } else {
                                DispatchProof::PossiblySent
                            },
                            ProviderFailureKind::ProtocolViolation,
                            "provider frame violated the admitted dialect",
                        )
                    })?;
                if !response_started && state.response_started {
                    response_started = true;
                    let observed = elapsed_timestamp(started, started_steady)?;
                    first_frame_at = Some(observed);
                    let provider_request_id =
                        adapter.request_id(&HeaderView::new(&headers), &state);
                    self.effects
                        .mark_response_started(
                            ticket,
                            &DispatchEvidence {
                                stage: DispatchStage::Streaming,
                                proof: DispatchProof::ResponseStarted,
                                attempt: ticket.attempt(),
                                provider_request_id,
                                external_operation: None,
                                detached_tool: None,
                                receipt: None,
                                detail: None,
                            },
                        )
                        .await
                        .map_err(|_| {
                            error(
                                DispatchStage::Streaming,
                                DispatchProof::ResponseStarted,
                                ProviderFailureKind::Transport,
                                "response-start evidence could not be committed",
                            )
                        })?;
                }
                match outcome {
                    FrameOutcome::Ignored => {}
                    FrameOutcome::ResponseStarted | FrameOutcome::Progress => {
                        let _ = preview.offer(PreviewFrame::InterimUsage(state.usage));
                    }
                    FrameOutcome::Terminal => terminal = true,
                    FrameOutcome::Failed(failure) => {
                        return Err(provider_failure(
                            DispatchStage::Streaming,
                            DispatchProof::ResponseStarted,
                            *failure,
                        ));
                    }
                }
            }
        }

        if !terminal {
            decoder
                .finish()
                .map_err(|failure| sse_error(failure, response_started))?;
        }
        let frames = state.ledger.frames;
        let provider_request_id = adapter.request_id(&HeaderView::new(&headers), &state);
        let assembled = adapter.finish(state).map_err(|_| {
            error(
                DispatchStage::Streaming,
                if response_started {
                    DispatchProof::ResponseStarted
                } else {
                    DispatchProof::PossiblySent
                },
                ProviderFailureKind::ProtocolViolation,
                "provider stream ended without a complete admitted response",
            )
        })?;
        let usage = assembled.usage;
        let message = seal(
            assembled.blocks,
            assembled.stop_reason,
            &usage,
            &request.selection,
        )
        .map_err(|_| {
            error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "decoded provider response could not be sealed",
            )
        })?;
        let rate_limit = adapter.rate_limit_feedback(&HeaderView::new(&headers));
        let completed_at = elapsed_timestamp(started, started_steady)?;
        let receipt = ProviderReceipt {
            provider: request.selection.provider(),
            model: request.selection.model().clone(),
            catalog: request.selection.catalog(),
            dialect: request.selection.dialect(),
            dialect_revision: request.selection.dialect_revision(),
            credential: CredentialBindingRef {
                id: binding.id,
                revision: binding.revision.0,
                generation: binding.generation.0,
            },
            provider_request_id: assembled.provider_request_id.or(provider_request_id),
            http_status,
            attempts,
            started_at: started,
            first_frame_at,
            completed_at,
            request_bytes,
            response_bytes,
            frames,
            rate_limit: Some(receipt_rate_limit(&rate_limit)),
            response_receipt: Some(message.proof.0),
            bounds: ReceiptBounds {
                max_frame_bytes: budget.max_frame_bytes,
                max_response_bytes: budget.max_response_bytes,
                idle_frame_timeout_ms: duration_ms_u32(budget.idle_frame_timeout),
                total_deadline_ms: duration_ms_u32(budget.total_deadline),
            },
        };
        let outcome = ProviderOutcome {
            message,
            usage,
            receipt,
        };
        if !outcome.is_consistent() {
            return Err(error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "the sealed provider outcome and receipt do not match",
            ));
        }
        Ok(outcome)
    }
}

impl ProviderPort for ProviderRouter {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        credential: SessionCredentialPin,
        request: &'a aex_model_catalog::canonical::CanonicalModelRequest,
        budget: &'a PortStreamBudget,
        preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(self.dispatch_pinned(ticket, credential, request, budget, preview, cancel))
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

fn adapter(provider: ProviderId) -> &'static dyn ProviderAdapter {
    match provider {
        ProviderId::Openai => &OpenAiAdapter,
        ProviderId::Anthropic => &AnthropicAdapter,
        ProviderId::Deepseek => &DeepSeekAdapter,
        ProviderId::Zai => &ZaiAdapter,
        ProviderId::Moonshotai => &MoonshotAdapter,
        ProviderId::Google => &GoogleAdapter,
    }
}

fn validate_request(
    ticket: &DispatchTicket,
    request: &aex_model_catalog::canonical::CanonicalModelRequest,
    adapter_source: AdapterSourceDigest,
) -> Result<(), ProviderDispatchError> {
    if request.correlation != CorrelationId::from_effect(ticket.effect().0) {
        return Err(error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::InvalidRequest,
            "the request correlation does not match the dispatch ticket effect",
        ));
    }
    if request.selection.state() != EntryState::Active {
        return Err(error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::ModelNotFound,
            "the catalog pair is not active",
        ));
    }
    if request.selection.entry().receipt.adapter_source != adapter_source {
        return Err(error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::InvalidRequest,
            "the catalog receipt does not match the build-stamped adapter source tree",
        ));
    }
    if !request.hash_is_consistent().unwrap_or(false) {
        return Err(error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::InvalidRequest,
            "the canonical provider request hash is inconsistent",
        ));
    }
    Ok(())
}

fn validate_binding(
    ticket: &DispatchTicket,
    provider: ProviderId,
    pin: SessionCredentialPin,
    binding: &crate::credential::ProviderCredentialBinding,
) -> Result<(), ProviderDispatchError> {
    if binding.workspace != ticket.workspace()
        || binding.context.workspace != ticket.workspace()
        || binding.context.organization != ticket.organization()
        || binding.context.generation != binding.generation
    {
        return Err(credential_error(CredentialResolveError::NotFound {
            requested: Some(pin.binding),
        }));
    }
    if binding.context.digest() != binding.context_digest {
        return Err(credential_error(CredentialResolveError::DecryptFailed));
    }
    if binding.id != pin.binding {
        return Err(credential_error(CredentialResolveError::NotFound {
            requested: Some(pin.binding),
        }));
    }
    if binding.provider != provider {
        return Err(credential_error(CredentialResolveError::ProviderMismatch {
            binding: binding.provider,
            requested: provider,
        }));
    }
    if binding.revision.0 != pin.revision.get() || binding.generation.0 != pin.generation.get() {
        return Err(credential_error(CredentialResolveError::NotFound {
            requested: Some(pin.binding),
        }));
    }
    match binding.state {
        BindingState::Ready => Ok(()),
        BindingState::Revoked => Err(credential_error(CredentialResolveError::Revoked {
            admitted: crate::wire_pending::RevocationEpoch(pin.revocation_epoch),
            current: binding.revocation_epoch,
        })),
        BindingState::Deleted => Err(credential_error(CredentialResolveError::Deleted)),
    }
}

fn validate_current_epoch(
    pin: SessionCredentialPin,
    binding: &crate::credential::ProviderCredentialBinding,
    current: crate::wire_pending::RevocationEpoch,
) -> Result<(), CredentialResolveError> {
    let admitted = crate::wire_pending::RevocationEpoch(pin.revocation_epoch);
    if current != admitted || current != binding.revocation_epoch {
        return Err(CredentialResolveError::Revoked { admitted, current });
    }
    Ok(())
}

fn gateway_budget(
    ticket: &DispatchTicket,
    port: &PortStreamBudget,
    request: &aex_model_catalog::canonical::CanonicalModelRequest,
) -> Result<StreamBudget, ProviderDispatchError> {
    let remaining_ms = port
        .deadline
        .millis()
        .saturating_sub(ticket.issued_at().millis());
    if remaining_ms <= 0 {
        return Err(error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::Timeout,
            "the provider dispatch deadline already elapsed",
        ));
    }
    let mut budget = StreamBudget {
        total_deadline: Duration::from_millis(remaining_ms.cast_unsigned()),
        idle_frame_timeout: Duration::from_millis(u64::from(port.idle_timeout_ms)),
        max_response_bytes: port.response_bytes as u64,
        max_frame_bytes: u32::try_from(port.buffer_bytes).unwrap_or(u32::MAX),
        ..StreamBudget::default()
    };
    budget = budget.narrowed_to(&request.selection);
    Ok(budget)
}

fn remaining(
    budget: &StreamBudget,
    started: Instant,
    proof: DispatchProof,
) -> Result<Duration, ProviderDispatchError> {
    budget
        .total_deadline
        .checked_sub(started.elapsed())
        .ok_or_else(|| {
            budget_error(
                BudgetOverrun::TotalDeadline {
                    after: budget.total_deadline,
                },
                proof,
            )
        })
}

/// Awaits one credential-authority step without letting pre-send work escape
/// the effect deadline or ignore cooperative cancellation indefinitely.
async fn await_pre_send<T, F>(
    future: F,
    budget: &StreamBudget,
    started: Instant,
    cancel: &CancelToken,
) -> Result<T, ProviderDispatchError>
where
    F: Future<Output = Result<T, CredentialResolveError>>,
{
    tokio::pin!(future);
    loop {
        if cancel.is_cancelled() {
            return Err(error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::Cancelled,
                "dispatch was cancelled while resolving provider credentials",
            ));
        }
        let left = remaining(budget, started, DispatchProof::NotSent)?;
        tokio::select! {
            result = &mut future => return result.map_err(credential_error),
            () = tokio::time::sleep(left) => {
                return Err(budget_error(
                    BudgetOverrun::TotalDeadline {
                        after: budget.total_deadline,
                    },
                    DispatchProof::NotSent,
                ));
            }
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

/// Awaits the mutable credential fence immediately before an upstream send.
/// `prior_proof` preserves the fact that a previous in-call retry attempt may
/// already have received a response.
async fn await_send_fence<T, F>(
    future: F,
    budget: &StreamBudget,
    started: Instant,
    cancel: &CancelToken,
    prior_proof: DispatchProof,
) -> Result<T, ProviderDispatchError>
where
    F: Future<Output = Result<T, CredentialResolveError>>,
{
    tokio::pin!(future);
    loop {
        if cancel.is_cancelled() {
            return Err(error(
                if prior_proof == DispatchProof::NotSent {
                    DispatchStage::PreDispatch
                } else {
                    DispatchStage::Terminal
                },
                prior_proof,
                ProviderFailureKind::Cancelled,
                "dispatch was cancelled while revalidating provider credentials",
            ));
        }
        let left = remaining(budget, started, prior_proof)?;
        tokio::select! {
            result = &mut future => {
                return result.map_err(|failure| credential_error_with_proof(failure, prior_proof));
            }
            () = tokio::time::sleep(left) => {
                return Err(budget_error(
                    BudgetOverrun::TotalDeadline {
                        after: budget.total_deadline,
                    },
                    prior_proof,
                ));
            }
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

async fn cancellable_sleep(
    duration: Duration,
    cancel: &CancelToken,
) -> Result<(), ProviderDispatchError> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.is_cancelled() {
            return Err(error(
                DispatchStage::Terminal,
                // Backoff is reachable only after a definitive non-generation
                // response, so the effect as a whole has already been sent.
                DispatchProof::ResponseStarted,
                ProviderFailureKind::Cancelled,
                "dispatch was cancelled during retry backoff",
            ));
        }
        tokio::time::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        )
        .await;
    }
    Ok(())
}

async fn read_error_body(
    response: reqwest::Response,
    budget: &StreamBudget,
    cancel: &CancelToken,
    key: &crate::credential::ProviderApiKey,
    started: Instant,
) -> BoundedBody {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut truncated = false;
    loop {
        if cancel.is_cancelled() {
            truncated = true;
            break;
        }
        let Some(left) = budget.total_deadline.checked_sub(started.elapsed()) else {
            truncated = true;
            break;
        };
        let timeout = budget.idle_frame_timeout.min(left);
        let Ok(chunk) = tokio::time::timeout(timeout, stream.next()).await else {
            truncated = true;
            break;
        };
        let Some(chunk) = chunk else {
            break;
        };
        let Ok(chunk) = chunk else {
            truncated = true;
            break;
        };
        let remaining = (budget.max_error_body_bytes as usize).saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    if let Ok(text) = core::str::from_utf8(&bytes) {
        let redacted = crate::redact::redact::<
            { crate::budget::DEFAULT_MAX_ERROR_BODY_BYTES as usize },
        >(text, &[key.expose_for_redaction()]);
        bytes = redacted.as_str().as_bytes().to_vec();
    }
    BoundedBody::new(bytes, truncated)
}

fn receipt_rate_limit(feedback: &RateLimitFeedback) -> ReceiptRateLimit {
    ReceiptRateLimit {
        retry_after_ms: feedback
            .retry_after
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        requests_remaining: feedback.requests_remaining,
        tokens_remaining: feedback.tokens_remaining,
        reset_at: feedback.reset_at,
        source: match feedback.source {
            RateLimitSource::NotProvided => ReceiptRateLimitSource::NotProvided,
            RateLimitSource::RetryAfterHeader => ReceiptRateLimitSource::RetryAfterHeader,
            RateLimitSource::VendorHeaders => ReceiptRateLimitSource::VendorHeaders,
            RateLimitSource::ErrorBody => ReceiptRateLimitSource::ErrorBody,
        },
    }
}

fn duration_ms_u32(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

fn wire_timestamp(
    value: aex_brain_domain::ids::Timestamp,
) -> Result<aex_wire::types::Timestamp, ProviderDispatchError> {
    aex_wire::types::Timestamp::from_unix_millis(value.millis()).map_err(|_| {
        error(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            ProviderFailureKind::InvalidRequest,
            "the dispatch timestamp is outside the wire range",
        )
    })
}

fn elapsed_timestamp(
    started: aex_wire::types::Timestamp,
    started_steady: Instant,
) -> Result<aex_wire::types::Timestamp, ProviderDispatchError> {
    // Derive receipt ordering from the same monotonic clock as the deadline.
    // An NTP correction during a stream must not turn a valid result into an
    // internally impossible `completed_at < started_at` receipt.
    let elapsed = i64::try_from(started_steady.elapsed().as_millis()).unwrap_or(i64::MAX);
    let millis = started.unix_millis().saturating_add(elapsed);
    aex_wire::types::Timestamp::from_unix_millis(millis).map_err(|_| {
        error(
            DispatchStage::Streaming,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "the monotonic receipt timestamp is outside the wire range",
        )
    })
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion is a direct map_err adapter"
)]
fn build_error(failure: RequestBuildError) -> ProviderDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        &failure.to_string(),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion is a direct map_err adapter"
)]
fn credential_error(failure: CredentialResolveError) -> ProviderDispatchError {
    credential_error_with_proof(failure, DispatchProof::NotSent)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion consumes the error into a redacted dispatch diagnostic"
)]
fn credential_error_with_proof(
    failure: CredentialResolveError,
    proof: DispatchProof,
) -> ProviderDispatchError {
    let kind = match &failure {
        CredentialResolveError::Transport => ProviderFailureKind::Transport,
        CredentialResolveError::RegistrationAuthorityUnavailable
        | CredentialResolveError::NotFound { .. }
        | CredentialResolveError::ProviderMismatch { .. }
        | CredentialResolveError::Revoked { .. }
        | CredentialResolveError::Deleted
        | CredentialResolveError::NoDefault { .. }
        | CredentialResolveError::AmbiguousDefault { .. }
        | CredentialResolveError::DecryptFailed => ProviderFailureKind::Authentication,
    };
    error(
        if proof == DispatchProof::NotSent {
            DispatchStage::PreDispatch
        } else {
            DispatchStage::Terminal
        },
        proof,
        kind,
        &failure.to_string(),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion is a direct map_err adapter"
)]
fn pool_error(failure: PoolError) -> ProviderDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::Transport,
        &failure.to_string(),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion is a direct map_err adapter"
)]
fn execute_error(failure: ExecuteError) -> ProviderDispatchError {
    let proof = failure.proof();
    let kind = match &failure {
        ExecuteError::Credential(_) => ProviderFailureKind::Authentication,
        ExecuteError::Transport { .. } => ProviderFailureKind::Transport,
        ExecuteError::Assembly(_) | ExecuteError::Header | ExecuteError::AlreadySent(_) => {
            ProviderFailureKind::InvalidRequest
        }
    };
    error(
        if proof == DispatchProof::NotSent {
            DispatchStage::PreDispatch
        } else {
            DispatchStage::Dispatched
        },
        proof,
        kind,
        &failure.to_string(),
    )
}

fn sse_error(failure: SseError, started: bool) -> ProviderDispatchError {
    error(
        DispatchStage::Streaming,
        if started {
            DispatchProof::ResponseStarted
        } else {
            DispatchProof::PossiblySent
        },
        ProviderFailureKind::ProtocolViolation,
        &failure.to_string(),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the small typed overrun is consumed at the failure-conversion boundary"
)]
fn budget_error(failure: BudgetOverrun, proof: DispatchProof) -> ProviderDispatchError {
    let kind = failure.kind();
    error(
        if proof == DispatchProof::NotSent {
            DispatchStage::PreDispatch
        } else {
            DispatchStage::Streaming
        },
        proof,
        kind,
        &failure.to_string(),
    )
}

fn provider_failure(
    stage: DispatchStage,
    proof: DispatchProof,
    failure: ProviderFailure,
) -> ProviderDispatchError {
    ProviderDispatchError {
        stage,
        proof,
        kind: failure.kind(),
        provider_request_id: None,
        retry_after: failure.rate_limit.retry_after,
        detail: failure.detail,
    }
}

fn error(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    detail: &str,
) -> ProviderDispatchError {
    ProviderDispatchError {
        stage,
        proof,
        kind,
        provider_request_id: None,
        retry_after: None,
        detail: RedactedDetail::new(kind, BoundedString::truncating(detail)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use aex_brain_application::ports::{CancelToken, DispatchTicket, FenceGuard};
    use aex_brain_domain::effect::DispatchProof;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, OwnerToken, SessionId,
        Timestamp,
    };
    use aex_brain_domain::wire_pending::SessionCredentialPin;
    use aex_model_catalog::ProviderFailureKind;
    use aex_secret_domain::context::Plane;
    use aex_secret_domain::{
        CiphertextRef, EncryptionContext, RevocationEpoch, SecretName, SourceGeneration,
    };
    use aex_wire::ids::{
        OrganizationId, PrefixedId as _, ProviderCredentialId, Uuid7, WorkspaceId,
    };
    use aex_wire::provider::ProviderId;
    use aex_wire::types::Region;
    use uuid::Uuid;

    use crate::budget::StreamBudget;
    use crate::credential::{
        BindingState, CredentialResolveError, CredentialRevision, ProviderCredentialBinding,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn organization(seed: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn credential() -> ProviderCredentialId {
        ProviderCredentialId::from_uuid7(Uuid7::compose(1, [3; 10]))
    }

    fn pin(epoch: u64) -> SessionCredentialPin {
        SessionCredentialPin::new(credential(), 2, 3, epoch).expect("non-zero fixture pin")
    }

    fn ticket(organization: OrganizationId) -> DispatchTicket {
        let guard = FenceGuard::new(
            AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
            OwnerToken(Uuid::from_u128(3)),
            Fence(1),
            AgentRevision(1),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            workspace(),
            organization,
            EffectId([4; 16]),
            1,
            Timestamp(0),
        )
    }

    fn binding(organization: OrganizationId, epoch: u64) -> ProviderCredentialBinding {
        let context = EncryptionContext {
            plane: Plane::Dev,
            region: Region::EuWest1,
            organization,
            workspace: workspace(),
            name: SecretName::parse("provider-key").expect("name"),
            generation: SourceGeneration(3),
            custody_revision: None,
        };
        ProviderCredentialBinding {
            id: credential(),
            workspace: workspace(),
            provider: ProviderId::Openai,
            revision: CredentialRevision(2),
            generation: SourceGeneration(3),
            revocation_epoch: RevocationEpoch(epoch),
            is_default: false,
            state: BindingState::Ready,
            ciphertext: CiphertextRef {
                key_generation: 1,
                wrapped_key: vec![1; 32],
                nonce: Vec::new(),
                ciphertext: vec![2; 32],
            },
            context_digest: context.digest(),
            context,
        }
    }

    #[test]
    fn all_six_providers_have_one_direct_adapter() {
        for provider in aex_wire::provider::ProviderId::ALL.iter().copied() {
            assert_eq!(super::adapter(provider).provider(), provider);
        }
    }

    #[test]
    fn organization_scope_mismatch_is_rejected_before_provider_io() {
        let error = super::validate_binding(
            &ticket(organization(5)),
            ProviderId::Openai,
            pin(7),
            &binding(organization(6), 7),
        )
        .expect_err("another organization's ciphertext context must be refused");
        assert_eq!(error.proof, DispatchProof::NotSent);
        assert_eq!(error.kind, ProviderFailureKind::Authentication);
    }

    #[test]
    fn a_revocation_observed_after_directory_resolution_wins_the_race() {
        let resolved = binding(organization(5), 7);
        let error = super::validate_current_epoch(pin(7), &resolved, RevocationEpoch(8))
            .expect_err("the second authority read observed revocation");
        assert_eq!(
            error,
            CredentialResolveError::Revoked {
                admitted: RevocationEpoch(7),
                current: RevocationEpoch(8),
            }
        );
    }

    #[test]
    fn exact_pin_and_revalidated_epoch_are_admitted() {
        let authority = organization(5);
        let resolved = binding(authority, 7);
        super::validate_binding(&ticket(authority), ProviderId::Openai, pin(7), &resolved)
            .expect("the immutable scope matches");
        super::validate_current_epoch(pin(7), &resolved, RevocationEpoch(7))
            .expect("the epoch did not move");
    }

    #[tokio::test]
    async fn a_hung_credential_authority_cannot_escape_the_effect_deadline() {
        let budget = StreamBudget {
            total_deadline: Duration::from_millis(1),
            ..StreamBudget::default()
        };
        let pending = futures::future::pending::<Result<(), CredentialResolveError>>();
        let error = super::await_pre_send(
            pending,
            &budget,
            Instant::now()
                .checked_sub(Duration::from_millis(2))
                .expect("two milliseconds fit within Instant's range"),
            &CancelToken::new(),
        )
        .await
        .expect_err("an elapsed deadline must stop the authority wait");
        assert_eq!(error.proof, DispatchProof::NotSent);
        assert_eq!(error.kind, ProviderFailureKind::Timeout);
    }

    #[tokio::test]
    async fn credential_authority_cancellation_remains_provably_unsent() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let pending = futures::future::pending::<Result<(), CredentialResolveError>>();
        let error =
            super::await_pre_send(pending, &StreamBudget::default(), Instant::now(), &cancel)
                .await
                .expect_err("cancellation must stop the authority wait");
        assert_eq!(error.proof, DispatchProof::NotSent);
        assert_eq!(error.kind, ProviderFailureKind::Cancelled);
    }

    #[tokio::test]
    async fn a_retry_fence_failure_preserves_the_prior_response_proof() {
        let error = super::await_send_fence(
            async {
                Err::<(), _>(CredentialResolveError::Revoked {
                    admitted: RevocationEpoch(7),
                    current: RevocationEpoch(8),
                })
            },
            &StreamBudget::default(),
            Instant::now(),
            &CancelToken::new(),
            DispatchProof::ResponseStarted,
        )
        .await
        .expect_err("a retry revalidation must fail closed");
        assert_eq!(error.proof, DispatchProof::ResponseStarted);
        assert_eq!(error.kind, ProviderFailureKind::Authentication);
    }

    #[test]
    fn receipt_times_are_monotonic_even_if_wall_time_moves() {
        let started = aex_wire::types::Timestamp::from_unix_millis(1_000).expect("timestamp");
        let completed = super::elapsed_timestamp(
            started,
            Instant::now()
                .checked_sub(Duration::from_millis(5))
                .expect("five milliseconds fit within Instant's range"),
        )
        .expect("elapsed timestamp");
        assert!(completed >= started);
    }
}
