//! The rig-backed `ProviderPort`: admission, credential resolution, bounded
//! retry, stream folding, sealing and receipt assembly.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use aex_brain_app::ports::proof::{CancelToken, DispatchTicket, PreviewSink};
use aex_brain_app::ports::provider::{ProviderOutcome, ProviderPort, UnknownResolution};
use aex_brain_app::ports::store::EffectStore;
use aex_brain_app::ports::{BoxFuture, ProviderDispatchError};
use aex_brain_domain::effect::{DispatchEvidence, DispatchProof, DispatchStage};
use aex_brain_domain::wire_pending::SessionCredentialPin;
use aex_brain_provider_custody::credential::{
    BindingState, CredentialResolveError, ProviderCredentialBinding, ProviderCredentialDecryptor,
    ProviderCredentialDirectory, RevocationEpoch,
};
use aex_model_catalog::canonical::{
    CanonicalModelRequest, CredentialBindingRef, ProviderReceipt, seal,
};
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::{ProviderFailureKind, RedactedDetail, admit};
use aex_model_vocabulary::DialectClass;
use aex_wire::provider::ProviderId;

use crate::cache::CredentialCache;
use crate::client::{ClientBuildError, DispatchClient};
use crate::response::{StreamFailure, consume_stream, is_retryable};

/// How many send attempts one dispatch may make.
const MAX_ATTEMPTS: u16 = 3;

/// The base equal-jitter backoff for a retried definitive rejection.
const RETRY_BASE_BACKOFF_MS: u64 = 250;

/// The cap on equal-jitter backoff.
const RETRY_MAX_BACKOFF_MS: u64 = 4_000;

/// Why a router cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderRouterBuildError {
    /// A compiled provider origin is not a valid base URL.
    #[error(transparent)]
    InvalidBaseUrl(#[from] ClientBuildError),
}

/// The one production `ProviderPort`: rig transport over the compiled
/// models.dev admit table, with the custody ports and effect store behind it.
pub struct RigProviderRouter {
    directory: Arc<dyn ProviderCredentialDirectory>,
    decryptor: Arc<dyn ProviderCredentialDecryptor>,
    effects: Arc<dyn EffectStore>,
    cache: CredentialCache,
    http: reqwest::Client,
    base_url_override: Option<String>,
}

impl core::fmt::Debug for RigProviderRouter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RigProviderRouter")
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl RigProviderRouter {
    /// Composes the router over the custody ports, the effect store, and one
    /// shared `reqwest::Client` for every provider dispatch.
    #[must_use]
    pub fn from_build(
        directory: Arc<dyn ProviderCredentialDirectory>,
        decryptor: Arc<dyn ProviderCredentialDecryptor>,
        effects: Arc<dyn EffectStore>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            directory,
            decryptor,
            effects,
            cache: CredentialCache::default(),
            http,
            base_url_override: None,
        }
    }

    /// Test-only: routes every dispatch at `base` instead of the compiled
    /// origin, so a local mock server can stand in for a provider.
    #[doc(hidden)]
    #[must_use]
    pub fn with_test_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url_override = Some(base.into());
        self
    }

    /// Retires every cached key for a binding.
    pub fn invalidate_binding(
        &self,
        workspace: aex_wire::ids::WorkspaceId,
        binding: aex_wire::ids::ProviderCredentialId,
    ) {
        let _ = self.cache.invalidate(workspace, binding);
    }

    async fn resolve_credential(
        &self,
        ticket: &DispatchTicket,
        pin: SessionCredentialPin,
        provider: ProviderId,
    ) -> Result<ProviderCredentialBinding, CredentialResolveError> {
        let binding = self
            .directory
            .resolve(
                ticket.organization(),
                ticket.workspace(),
                provider,
                Some(pin.binding),
            )
            .await?;
        if binding.provider != provider {
            return Err(CredentialResolveError::ProviderMismatch {
                binding: binding.provider,
                requested: provider,
            });
        }
        match binding.state {
            BindingState::Deleted => return Err(CredentialResolveError::Deleted),
            BindingState::Revoked => {
                return Err(CredentialResolveError::Revoked {
                    admitted: RevocationEpoch(pin.revocation_epoch),
                    current: binding.revocation_epoch,
                });
            }
            BindingState::Ready => {}
        }
        if binding.revocation_epoch > RevocationEpoch(pin.revocation_epoch) {
            return Err(CredentialResolveError::Revoked {
                admitted: RevocationEpoch(pin.revocation_epoch),
                current: binding.revocation_epoch,
            });
        }
        Ok(binding)
    }
}

impl ProviderPort for RigProviderRouter {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        credential: SessionCredentialPin,
        request: &'a CanonicalModelRequest,
        preview: &'a dyn PreviewSink,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async move {
            self.dispatch_inner(ticket, credential, request, preview, cancel)
                .await
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a aex_brain_domain::effect::DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        Box::pin(async move { Ok(UnknownResolution::NoDurableOperation) })
    }
}

impl RigProviderRouter {
    #[allow(
        clippy::too_many_lines,
        reason = "the linear send-proof state machine is kept together so every await and retry visibly preserves its dispatch proof"
    )]
    async fn dispatch_inner(
        &self,
        ticket: &DispatchTicket,
        pin: SessionCredentialPin,
        request: &CanonicalModelRequest,
        preview: &dyn PreviewSink,
        cancel: &CancelToken,
    ) -> Result<ProviderOutcome, ProviderDispatchError> {
        let started = wire_timestamp(ticket.issued_at()).map_err(|error| {
            failure(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                &error.to_string(),
            )
        })?;
        let started_steady = Instant::now();

        // The request hash is the pre-send integrity fence.
        if !request.hash_is_consistent().unwrap_or(false) {
            return Err(failure(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                "the canonical request hash does not cover the request",
            ));
        }

        // Compiled-table admission: a pair the table does not carry is never
        // sent, whatever a request asks for.
        let qualified = admit(
            request.selection.provider(),
            request.selection.model().as_str(),
        )
        .map_err(|error| {
            failure(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::ModelNotFound,
                &error.to_string(),
            )
        })?;
        let provider = qualified.provider();

        // Credential resolution and the pre-send revalidation fence.
        let binding = self
            .resolve_credential(ticket, pin, provider)
            .await
            .map_err(|error| {
                failure(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    credential_kind(&error),
                    &error.to_string(),
                )
            })?;
        let key = self
            .cache
            .decrypt(&binding, self.decryptor.as_ref(), started)
            .await
            .map_err(|error| {
                failure(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    credential_kind(&error),
                    &error.to_string(),
                )
            })?;
        self.directory.revalidate(&binding).await.map_err(|error| {
            failure(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                credential_kind(&error),
                &error.to_string(),
            )
        })?;

        let rig_request = crate::request::build(request, qualified.dialect()).map_err(|error| {
            failure(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                &error.to_string(),
            )
        })?;

        // The retry loop: only a definitive 429/503 may be re-sent; anything
        // ambiguous or started terminates immediately.
        let response_started = AtomicBool::new(false);
        let mut attempts: u16 = 0;
        let evidence = DispatchEvidence {
            stage: DispatchStage::Streaming,
            proof: DispatchProof::ResponseStarted,
            attempt: ticket.attempt(),
            provider_request_id: None,
            external_operation: None,
            detached_tool: None,
            receipt: None,
            detail: None,
        };
        let marked = Arc::new(AtomicBool::new(false));
        let outcome = loop {
            if cancel.is_cancelled() {
                let started = response_started.load(std::sync::atomic::Ordering::Relaxed);
                return Err(failure(
                    if started {
                        DispatchStage::Streaming
                    } else {
                        DispatchStage::PreDispatch
                    },
                    if started {
                        DispatchProof::PossiblySent
                    } else {
                        DispatchProof::NotSent
                    },
                    ProviderFailureKind::Cancelled,
                    "the dispatch was cancelled",
                ));
            }
            attempts += 1;

            let client = DispatchClient::build(
                qualified.dialect(),
                self.base_url_override
                    .as_deref()
                    .unwrap_or_else(|| qualified.base_url()),
                key.plaintext(),
                &self.http,
            )
            .map_err(|error| {
                failure(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    &error.to_string(),
                )
            })?;

            let mark = {
                let effects = Arc::clone(&self.effects);
                let marked = Arc::clone(&marked);
                let evidence = &evidence;
                move || {
                    let effects = Arc::clone(&effects);
                    let marked = Arc::clone(&marked);
                    async move {
                        if marked.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            return Ok(());
                        }
                        effects
                            .mark_response_started(ticket, evidence)
                            .await
                            .map_err(|_| StreamFailure::EffectStore {
                                detail: "the durable response-started write failed".to_owned(),
                            })
                    }
                }
            };
            let stream_result = dispatch_once(
                &client,
                qualified.dialect(),
                qualified.model().as_str(),
                provider,
                &rig_request,
                preview,
                cancel,
                &response_started,
                mark,
            )
            .await;

            match stream_result {
                Ok(stream_outcome) => break stream_outcome,
                Err(StreamFailure::Definitive { status, detail }) if is_retryable(status) => {
                    if attempts >= MAX_ATTEMPTS {
                        return Err(rejection_failure(status, &detail));
                    }
                    sleep_backoff(attempts).await;
                }
                Err(StreamFailure::Definitive { status, detail }) => {
                    return Err(rejection_failure(status, &detail));
                }
                Err(StreamFailure::Transport { detail }) => {
                    return Err(failure(
                        DispatchStage::Terminal,
                        DispatchProof::PossiblySent,
                        ProviderFailureKind::Transport,
                        &detail,
                    ));
                }
                Err(StreamFailure::Cancelled) => {
                    return Err(failure(
                        DispatchStage::Streaming,
                        DispatchProof::PossiblySent,
                        ProviderFailureKind::Cancelled,
                        "the dispatch was cancelled mid-stream",
                    ));
                }
                Err(StreamFailure::Protocol { detail }) => {
                    return Err(failure(
                        DispatchStage::Terminal,
                        DispatchProof::ResponseStarted,
                        ProviderFailureKind::ProtocolViolation,
                        &detail,
                    ));
                }
                Err(StreamFailure::EffectStore { detail }) => {
                    return Err(failure(
                        DispatchStage::Terminal,
                        DispatchProof::ResponseStarted,
                        ProviderFailureKind::ServerError,
                        &detail,
                    ));
                }
            }
        };

        // Seal: the assembled blocks become a complete assistant message.
        let message =
            seal(outcome.blocks, outcome.stop, &outcome.usage, &qualified).map_err(|_| {
                failure(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "the decoded provider response could not be sealed",
                )
            })?;
        let first_frame_at = outcome
            .first_item_at
            .map(|instant| elapsed_timestamp(started, instant))
            .transpose()?;
        let completed_at = elapsed_timestamp(started, started_steady)?;
        let receipt = ProviderReceipt {
            provider,
            model: qualified.model().clone(),
            catalog: qualified.catalog(),
            dialect: qualified.dialect(),
            credential: CredentialBindingRef {
                id: binding.id,
                revision: binding.revision.0,
                generation: binding.generation.0,
            },
            provider_request_id: None,
            gateway_route: None,
            http_status: 200,
            attempts,
            started_at: started,
            first_frame_at,
            completed_at,
            rate_limit: None,
            response_receipt: Some(message.proof.0),
        };
        let outcome = ProviderOutcome {
            message,
            usage: outcome.usage,
            receipt,
        };
        if !outcome.is_consistent() {
            return Err(failure(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "the sealed provider outcome and receipt do not match",
            ));
        }
        Ok(outcome)
    }
}

/// One attempt: build the model handle and drive the stream. `on_started`
/// fires once per dispatch, on the first decoded item, and commits the
/// durable `ResponseStarted` evidence.
#[allow(clippy::too_many_arguments, reason = "one dispatch: client, request and stream sinks together")]
async fn dispatch_once<F, Fut>(
    client: &DispatchClient,
    dialect: DialectClass,
    model_id: &str,
    provider: ProviderId,
    request: &rig_core::completion::CompletionRequest,
    preview: &dyn PreviewSink,
    cancel: &CancelToken,
    response_started: &AtomicBool,
    mut on_started: F,
) -> Result<crate::response::StreamOutcome, StreamFailure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), StreamFailure>>,
{
    use rig_core::client::CompletionClient as _;
    match client {
        DispatchClient::OpenAi(client) => {
            consume_stream(
                &client.completion_model(model_id),
                request,
                dialect,
                provider,
                preview,
                cancel,
                response_started,
                &mut on_started,
            )
            .await
        }
        DispatchClient::Anthropic(client) => {
            consume_stream(
                &client.completion_model(model_id),
                request,
                dialect,
                provider,
                preview,
                cancel,
                response_started,
                &mut on_started,
            )
            .await
        }
        DispatchClient::Gemini(client) => {
            consume_stream(
                &client.completion_model(model_id),
                request,
                dialect,
                provider,
                preview,
                cancel,
                response_started,
                &mut on_started,
            )
            .await
        }
        DispatchClient::Compatible(client) => {
            consume_stream(
                &client.completion_model(model_id),
                request,
                dialect,
                provider,
                preview,
                cancel,
                response_started,
                &mut on_started,
            )
            .await
        }
    }
}

/// The equal-jitter backoff between retried definitive rejections.
async fn sleep_backoff(attempt: u16) {
    use rand::RngExt as _;
    let step = RETRY_BASE_BACKOFF_MS.saturating_mul(1u64 << attempt.min(4));
    let capped = step.min(RETRY_MAX_BACKOFF_MS);
    let half = capped / 2;
    let jitter = rand::rng().random_range(0..=half);
    tokio::time::sleep(core::time::Duration::from_millis(half + jitter)).await;
}

/// The ticket's Brain timestamp, in the wire range.
fn wire_timestamp(
    timestamp: aex_brain_domain::ids::Timestamp,
) -> Result<aex_wire::types::Timestamp, aex_wire::types::ValueError> {
    aex_wire::types::Timestamp::from_unix_millis(timestamp.0)
}

/// Converts a steady elapsed duration into a wire timestamp after `started`.
fn elapsed_timestamp(
    started: aex_wire::types::Timestamp,
    started_steady: Instant,
) -> Result<aex_wire::types::Timestamp, ProviderDispatchError> {
    let elapsed_ms = i64::try_from(started_steady.elapsed().as_millis()).unwrap_or(i64::MAX);
    aex_wire::types::Timestamp::from_unix_millis(started.unix_millis().saturating_add(elapsed_ms))
        .map_err(|_| {
            failure(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::InvalidRequest,
                "the wire clock ran out of range mid-dispatch",
            )
        })
}

/// The canonical failure kind a credential error renders as.
fn credential_kind(error: &CredentialResolveError) -> ProviderFailureKind {
    match error {
        CredentialResolveError::Transport => ProviderFailureKind::Transport,
        _ => ProviderFailureKind::Authentication,
    }
}

/// The error for a definitive provider rejection, after retries if any.
fn rejection_failure(status: u16, detail: &str) -> ProviderDispatchError {
    let kind = match status {
        429 => ProviderFailureKind::RateLimited,
        503 => ProviderFailureKind::Overloaded,
        401 | 403 => ProviderFailureKind::Authentication,
        400 => ProviderFailureKind::InvalidRequest,
        _ => ProviderFailureKind::ServerError,
    };
    ProviderDispatchError {
        stage: DispatchStage::Terminal,
        proof: DispatchProof::ResponseStarted,
        kind,
        provider_request_id: None,
        retry_after: None,
        detail: RedactedDetail {
            kind,
            http_status: Some(status),
            provider_code: None,
            message: BoundedString::truncating(detail),
        },
    }
}

/// A dispatch failure with a redacted detail.
fn failure(
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
        detail: RedactedDetail {
            kind,
            http_status: None,
            provider_code: None,
            message: BoundedString::truncating(detail),
        },
    }
}
