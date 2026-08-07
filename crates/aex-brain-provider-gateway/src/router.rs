//! Bounded direct-provider router over the six admitted BYOK dialects.

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

    /// Dispatches with the immutable credential binding a session admitted.
    ///
    /// This is the complete transport path waiting behind the Brain session
    /// contract. The current `ProviderPort` request does not carry this pin, so
    /// its implementation below fails closed before reaching this method.
    ///
    /// # Errors
    ///
    /// Returns a typed provider dispatch failure with an exact send proof.
    #[allow(
        dead_code,
        reason = "the complete transport is intentionally unreachable until the public Brain session contract carries its immutable credential pin"
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
        if cancel.is_cancelled() {
            return Err(error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::Cancelled,
                "dispatch was cancelled before send",
            ));
        }

        let provider = request.selection.provider();
        let binding = self
            .directory
            .resolve(ticket.workspace(), provider, Some(pin.binding))
            .await
            .map_err(credential_error)?;
        validate_binding(ticket, provider, pin, &binding)?;
        let current_epoch = self
            .directory
            .current_epoch(ticket.workspace(), binding.id)
            .await
            .map_err(credential_error)?;
        if current_epoch != pin.epoch_at_admission || current_epoch != binding.revocation_epoch {
            self.cache.invalidate(ticket.workspace(), binding.id);
            self.pool.close(&|key| {
                key.workspace == ticket.workspace()
                    && key.credential_generation == binding.generation
            });
            return Err(credential_error(CredentialResolveError::Revoked {
                admitted: pin.epoch_at_admission,
                current: current_epoch,
            }));
        }

        let key = self
            .cache
            .decrypt(&binding, self.decryptor.as_ref())
            .await
            .map_err(credential_error)?;
        let budget = gateway_budget(ticket, port_budget, request)?;
        let pooled = self
            .pool
            .acquire(
                &IsolationKey {
                    origin: request.selection.endpoint(),
                    workspace: ticket.workspace(),
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

        let started = wire_timestamp(ticket.issued_at())?;
        let started_steady = Instant::now();
        let mut attempt = 0_u16;
        loop {
            attempt = attempt.saturating_add(1);
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
                    let observed = wire_now()?;
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
                                operation: None,
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
        let decoded = adapter.finish(state).map_err(|_| {
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
        let usage = decoded.usage;
        let message = seal(
            decoded.blocks,
            decoded.stop_reason,
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
        let completed_at = wire_now()?;
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
            provider_request_id: decoded.provider_request_id.or(provider_request_id),
            http_status,
            attempts,
            started_at: started,
            first_frame_at,
            completed_at,
            request_bytes,
            response_bytes,
            frames,
            rate_limit: Some(receipt_rate_limit(rate_limit)),
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
        _ticket: &'a DispatchTicket,
        _request: &'a aex_model_catalog::canonical::CanonicalModelRequest,
        _budget: &'a PortStreamBudget,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async {
            Err(error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                "the Brain session contract carries no immutable provider credential pin",
            ))
        })
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
    if binding.workspace != ticket.workspace() {
        return Err(credential_error(CredentialResolveError::NotFound {
            requested: Some(pin.binding),
        }));
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
    if binding.revision != pin.revision || binding.generation != pin.generation {
        return Err(credential_error(CredentialResolveError::NotFound {
            requested: Some(pin.binding),
        }));
    }
    match binding.state {
        BindingState::Ready => Ok(()),
        BindingState::Revoked => Err(credential_error(CredentialResolveError::Revoked {
            admitted: pin.epoch_at_admission,
            current: binding.revocation_epoch,
        })),
        BindingState::Deleted => Err(credential_error(CredentialResolveError::Deleted)),
    }
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
        total_deadline: Duration::from_millis(remaining_ms as u64),
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

fn receipt_rate_limit(feedback: RateLimitFeedback) -> ReceiptRateLimit {
    ReceiptRateLimit {
        retry_after_ms: feedback
            .retry_after
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
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

fn wire_now() -> Result<aex_wire::types::Timestamp, ProviderDispatchError> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            error(
                DispatchStage::Streaming,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "the system clock is before the Unix epoch",
            )
        })?
        .as_millis();
    let millis = i64::try_from(millis).map_err(|_| {
        error(
            DispatchStage::Streaming,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "the system clock is outside the wire range",
        )
    })?;
    aex_wire::types::Timestamp::from_unix_millis(millis).map_err(|_| {
        error(
            DispatchStage::Streaming,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "the system clock is outside the wire range",
        )
    })
}

fn build_error(failure: RequestBuildError) -> ProviderDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        &failure.to_string(),
    )
}

fn credential_error(failure: CredentialResolveError) -> ProviderDispatchError {
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
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        kind,
        &failure.to_string(),
    )
}

fn pool_error(failure: PoolError) -> ProviderDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::Transport,
        &failure.to_string(),
    )
}

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
    #[test]
    fn all_six_providers_have_one_direct_adapter() {
        for provider in aex_wire::provider::ProviderId::ALL.iter().copied() {
            assert_eq!(super::adapter(provider).provider(), provider);
        }
    }
}
