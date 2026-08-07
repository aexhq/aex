//! Reusable bounded consumption of an already-dispatched provider stream.
//!
//! The production router and the live conformance harness need the same SSE
//! decoder, dialect state machine, byte accounting, timeouts and dispatch
//! proof. This module owns that post-response-head seam. It deliberately does
//! not own endpoint selection, credentials, request construction or sending,
//! so it cannot introduce an arbitrary origin or a second generation.

use std::time::{Duration, Instant};

use aex_brain_app::ports::{BoxFuture, CancelToken, PreviewSink};
use aex_brain_domain::effect::DispatchProof;
use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::PreviewFrame;
use aex_model_catalog::document::{Capability, Dialect, EndpointPin, EntryState, ModelEntry};
use aex_model_catalog::primitives::ProviderRequestId;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};

use crate::adapter::{FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter, SealedResponse};
use crate::budget::{BudgetOverrun, StreamBudget};
use crate::error::{ProviderFailure, ProviderFailureKind};
use crate::sse::{SseDecoder, SseError};

/// A sink for the one durable transition caused by the first validated frame.
///
/// The stream consumer awaits this write before consuming another frame. A
/// no-op implementation exists for live probes, while `ProviderRouter` binds
/// the production effect store.
pub trait ResponseStartSink: Send + Sync {
    /// Persists or observes the first validated dialect frame.
    fn mark(
        &self,
        provider_request_id: Option<ProviderRequestId>,
    ) -> BoxFuture<'_, Result<(), ResponseStartSinkError>>;
}

/// The response-start observer could not commit its evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("response-start evidence could not be committed")]
pub struct ResponseStartSinkError;

/// A response-start sink for callers that have no durable effect authority.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullResponseStartSink;

impl ResponseStartSink for NullResponseStartSink {
    fn mark(
        &self,
        _provider_request_id: Option<ProviderRequestId>,
    ) -> BoxFuture<'_, Result<(), ResponseStartSinkError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Exact protocol failure observed while consuming a provider stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamProtocolError {
    /// The generic SSE framing was invalid.
    #[error("provider SSE framing failed: {0}")]
    Sse(#[from] SseError),
    /// The provider dialect rejected a frame or incomplete terminal state.
    #[error("provider dialect decoding failed: {0}")]
    Frame(#[from] FrameDecodeError),
}

/// Why an already-dispatched stream did not produce a sealed response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamFailure {
    /// Cooperative cancellation was observed.
    #[error("provider stream was cancelled")]
    Cancelled,
    /// A byte or time budget was crossed.
    #[error("provider stream crossed a bound: {0}")]
    Budget(#[from] BudgetOverrun),
    /// The response body transport failed.
    #[error("provider stream transport failed")]
    Transport,
    /// SSE framing or provider dialect decoding failed.
    #[error("{0}")]
    Protocol(#[from] StreamProtocolError),
    /// The provider reported a definitive in-stream failure.
    #[error("provider reported a definitive in-stream failure")]
    Provider(Box<ProviderFailure>),
    /// The caller could not persist response-start evidence.
    #[error("{0}")]
    ResponseStart(#[from] ResponseStartSinkError),
}

/// A failed stream plus the exact evidence needed for recovery classification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{failure}")]
pub struct StreamConsumeError {
    /// The immediate failure.
    pub failure: StreamFailure,
    /// Whether a validated dialect frame proved generation had started.
    pub response_started: bool,
    /// Provider correlation observed before the failure, where available.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Response-body bytes consumed before the failure.
    pub response_bytes: u64,
    /// Dialect frames decoded before the failure.
    pub frames: u32,
    /// Frames decoded after cooperative cancellation was observed.
    ///
    /// The consumer currently settles immediately at that observation, so a
    /// cancellation failure must carry zero here.
    pub frames_after_cancel: u32,
}

impl StreamConsumeError {
    /// The durable dispatch proof implied by this post-send failure.
    #[must_use]
    pub const fn proof(&self) -> DispatchProof {
        if self.response_started {
            DispatchProof::ResponseStarted
        } else {
            DispatchProof::PossiblySent
        }
    }

    /// The normalized operational failure kind.
    #[must_use]
    pub fn kind(&self) -> ProviderFailureKind {
        match &self.failure {
            StreamFailure::Cancelled => ProviderFailureKind::Cancelled,
            StreamFailure::Budget(failure) => failure.kind(),
            StreamFailure::Transport | StreamFailure::ResponseStart(_) => {
                ProviderFailureKind::Transport
            }
            StreamFailure::Protocol(_) => ProviderFailureKind::ProtocolViolation,
            StreamFailure::Provider(failure) => failure.kind(),
        }
    }
}

/// A complete decoded stream before the production router seals and receipts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedStream {
    /// Provider-neutral decoded response.
    pub response: SealedResponse,
    /// Provider correlation from headers or the decoded body.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Elapsed time to the first validated dialect frame.
    pub first_frame_after: Option<Duration>,
    /// Total response-body bytes consumed.
    pub response_bytes: u64,
    /// Dialect frames decoded.
    pub frames: u32,
}

/// Why the qualification-only staged-entry stream seam was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QualificationStreamError {
    /// The caller did not provide a staged direct `DeepSeek` streaming entry.
    #[error("the qualification stream requires a staged direct DeepSeek streaming entry")]
    Candidate,
    /// The production stream consumer rejected or could not complete the body.
    #[error(transparent)]
    Consume(#[from] StreamConsumeError),
}

/// Consumes one already-dispatched provider body under the production bounds.
///
/// `body` is generic so a live conformance runner can wrap a real provider
/// stream with deterministic drop, oversize and idle faults. The request still
/// has to pass the ordinary pinned endpoint, credential and send path first.
///
/// # Errors
///
/// Returns [`StreamConsumeError`] with `PossiblySent` or `ResponseStarted`
/// proof. This function is unreachable before the caller has a response head,
/// so it can never claim `NotSent`.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the post-send state machine keeps every bound, decoded frame, and proof transition in one auditable sequence"
)]
pub async fn consume_provider_stream<S, E>(
    adapter: &dyn ProviderAdapter,
    model: &QualifiedModel,
    body: S,
    headers: HeaderView<'_>,
    budget: &StreamBudget,
    preview: &dyn PreviewSink,
    cancel: &CancelToken,
    started: Instant,
    response_start: &dyn ResponseStartSink,
) -> Result<ConsumedStream, StreamConsumeError>
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: Send,
{
    consume_provider_stream_core(
        adapter,
        adapter.new_state(model),
        body,
        headers,
        budget,
        preview,
        cancel,
        started,
        response_start,
    )
    .await
}

/// Consumes a staged direct `DeepSeek` body through the production stream core.
///
/// This is the only seam that permits a not-yet-qualified [`ModelEntry`] to
/// exercise runtime-equivalent decoding. It admits no free-form origin or
/// dialect and cannot make the staged entry routable in production.
///
/// # Errors
///
/// Returns [`QualificationStreamError::Candidate`] unless the entry is a
/// staged direct `DeepSeek` text-streaming candidate, and otherwise propagates
/// the production consumer's typed failure and dispatch proof.
#[allow(clippy::too_many_arguments)]
pub async fn consume_deepseek_qualification_stream<S, E>(
    entry: &ModelEntry,
    body: S,
    headers: HeaderView<'_>,
    budget: &StreamBudget,
    preview: &dyn PreviewSink,
    cancel: &CancelToken,
    started: Instant,
    response_start: &dyn ResponseStartSink,
) -> Result<ConsumedStream, QualificationStreamError>
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: Send,
{
    if entry.provider != aex_wire::provider::ProviderId::Deepseek
        || entry.state != EntryState::Staged
        || entry.dialect != Dialect::DeepSeekChat
        || entry.endpoint != EndpointPin::DeepSeekApi
        || !entry.capabilities.has(Capability::TextIn)
        || !entry.capabilities.has(Capability::TextOut)
        || !entry.capabilities.has(Capability::Streaming)
        || entry.limits.context_window_tokens == 0
        || entry.limits.max_output_tokens < entry.limits.min_output_tokens
    {
        return Err(QualificationStreamError::Candidate);
    }
    consume_provider_stream_core(
        &crate::deepseek::DeepSeekAdapter,
        crate::adapter::DialectState::new(),
        body,
        headers,
        budget,
        preview,
        cancel,
        started,
        response_start,
    )
    .await
    .map_err(Into::into)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the post-send state machine keeps every bound, decoded frame, and proof transition in one auditable sequence"
)]
async fn consume_provider_stream_core<S, E>(
    adapter: &dyn ProviderAdapter,
    mut state: crate::adapter::DialectState,
    body: S,
    headers: HeaderView<'_>,
    budget: &StreamBudget,
    preview: &dyn PreviewSink,
    cancel: &CancelToken,
    started: Instant,
    response_start: &dyn ResponseStartSink,
) -> Result<ConsumedStream, StreamConsumeError>
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: Send,
{
    let mut body = Box::pin(body);
    let mut decoder = SseDecoder::new(budget.max_frame_bytes);
    let mut response_bytes = 0_u64;
    let mut first_frame_after = None;
    let mut response_started = false;
    let mut terminal = false;

    while !terminal {
        if cancel.is_cancelled() {
            return Err(stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Cancelled,
            ));
        }
        let timeout = if response_started {
            budget.idle_frame_timeout
        } else {
            budget.first_frame_timeout
        }
        .min(remaining(budget, started).map_err(|failure| {
            stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Budget(failure),
            )
        })?);
        let chunk = tokio::time::timeout(timeout, body.next())
            .await
            .map_err(|_| {
                let failure = if response_started {
                    BudgetOverrun::IdleFrame { after: timeout }
                } else {
                    BudgetOverrun::FirstFrame { after: timeout }
                };
                stream_error(
                    adapter,
                    headers,
                    &state,
                    response_bytes,
                    StreamFailure::Budget(failure),
                )
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|_| {
            stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Transport,
            )
        })?;
        response_bytes = response_bytes.saturating_add(chunk.len() as u64);
        if response_bytes > budget.max_response_bytes {
            return Err(stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Budget(BudgetOverrun::Response {
                    limit: budget.max_response_bytes,
                }),
            ));
        }
        decoder.push(&chunk).map_err(|failure| {
            stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Protocol(StreamProtocolError::Sse(failure)),
            )
        })?;
        for event in decoder.drain().map_err(|failure| {
            stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Protocol(StreamProtocolError::Sse(failure)),
            )
        })? {
            let outcome = adapter
                .decode(&mut state, &event.as_ref(), budget)
                .map_err(|failure| {
                    stream_error(
                        adapter,
                        headers,
                        &state,
                        response_bytes,
                        StreamFailure::Protocol(StreamProtocolError::Frame(failure)),
                    )
                })?;
            if !response_started && state.response_started {
                response_started = true;
                first_frame_after = Some(started.elapsed());
                let request_id = adapter.request_id(&headers, &state);
                response_start.mark(request_id).await.map_err(|failure| {
                    stream_error(
                        adapter,
                        headers,
                        &state,
                        response_bytes,
                        StreamFailure::ResponseStart(failure),
                    )
                })?;
            }
            if cancel.is_cancelled() {
                return Err(stream_error(
                    adapter,
                    headers,
                    &state,
                    response_bytes,
                    StreamFailure::Cancelled,
                ));
            }
            match outcome {
                FrameOutcome::Ignored => {}
                FrameOutcome::ResponseStarted | FrameOutcome::Progress => {
                    let _ = preview.offer(PreviewFrame::InterimUsage(state.usage));
                }
                FrameOutcome::Terminal => terminal = true,
                FrameOutcome::Failed(failure) => {
                    return Err(stream_error(
                        adapter,
                        headers,
                        &state,
                        response_bytes,
                        StreamFailure::Provider(failure),
                    ));
                }
            }
        }
    }

    if !terminal {
        decoder.finish().map_err(|failure| {
            stream_error(
                adapter,
                headers,
                &state,
                response_bytes,
                StreamFailure::Protocol(StreamProtocolError::Sse(failure)),
            )
        })?;
    }
    let frames = state.ledger.frames;
    let provider_request_id = adapter.request_id(&headers, &state);
    let response = adapter
        .finish(state)
        .map_err(|failure| StreamConsumeError {
            failure: StreamFailure::Protocol(StreamProtocolError::Frame(failure)),
            response_started,
            provider_request_id: provider_request_id.clone(),
            response_bytes,
            frames,
            frames_after_cancel: 0,
        })?;
    Ok(ConsumedStream {
        response,
        provider_request_id,
        first_frame_after,
        response_bytes,
        frames,
    })
}

fn remaining(budget: &StreamBudget, started: Instant) -> Result<Duration, BudgetOverrun> {
    budget
        .total_deadline
        .checked_sub(started.elapsed())
        .ok_or(BudgetOverrun::TotalDeadline {
            after: budget.total_deadline,
        })
}

fn stream_error(
    adapter: &dyn ProviderAdapter,
    headers: HeaderView<'_>,
    state: &crate::adapter::DialectState,
    response_bytes: u64,
    failure: StreamFailure,
) -> StreamConsumeError {
    StreamConsumeError {
        response_started: state.response_started,
        provider_request_id: adapter.request_id(&headers, state),
        response_bytes,
        frames: state.ledger.frames,
        frames_after_cancel: 0,
        failure,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use aex_brain_app::ports::{BoxFuture, CancelToken, NullPreviewSink};
    use aex_brain_domain::effect::DispatchProof;
    use aex_model_catalog::canonical::{CanonicalBlock, CanonicalModelRequest, StopReason};
    use aex_model_catalog::document::{Capability, CapabilitySet, EntryState};
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::{BoundedString, ProviderRequestId};
    use aex_model_catalog::{ProviderFailureKind, QualifiedModel, RedactedDetail};
    use aex_wire::provider::ProviderId;
    use bytes::Bytes;
    use futures::stream;

    use super::{
        ConsumedStream, NullResponseStartSink, QualificationStreamError, ResponseStartSink,
        ResponseStartSinkError, StreamFailure, consume_deepseek_qualification_stream,
        consume_provider_stream,
    };
    use crate::adapter::{
        BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter,
        RequestBuildError, SealedResponse,
    };
    use crate::budget::StreamBudget;
    use crate::error::{ProviderFailure, RateLimitFeedback};
    use crate::sse::SseEvent;
    use crate::transport::WireRequest;

    #[derive(Debug, Clone, Copy)]
    struct FixtureAdapter;

    impl ProviderAdapter for FixtureAdapter {
        fn provider(&self) -> ProviderId {
            ProviderId::Openai
        }

        fn build_request(
            &self,
            _model: &QualifiedModel,
            _request: &CanonicalModelRequest,
        ) -> Result<WireRequest, RequestBuildError> {
            unreachable!("stream-consumer tests start after request construction")
        }

        fn new_state(&self, _model: &QualifiedModel) -> DialectState {
            DialectState::new()
        }

        fn decode(
            &self,
            state: &mut DialectState,
            event: &SseEvent<'_>,
            _budget: &StreamBudget,
        ) -> Result<FrameOutcome, FrameDecodeError> {
            state.ledger.count_frame();
            match event.data_str().map_err(|_| FrameDecodeError::NotJson)? {
                "start" => Ok(state.mark_started()),
                "done" => {
                    let outcome = state.mark_started();
                    state.blocks.push(CanonicalBlock::Text {
                        text: BoundedString::new("ok").expect("fixture text"),
                        annotations: Vec::new(),
                    });
                    state.finish_token = Some("stop".to_owned());
                    state.terminal = true;
                    let _ = outcome;
                    Ok(FrameOutcome::Terminal)
                }
                _ => Err(FrameDecodeError::UnknownEvent {
                    event: BoundedString::new("fixture").expect("fixture event"),
                }),
            }
        }

        fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
            if !state.terminal {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "fixture stream has no terminal frame",
                });
            }
            Ok(SealedResponse {
                blocks: state.blocks,
                stop_reason: StopReason::EndTurn,
                usage: state.usage,
                provider_request_id: Some(
                    ProviderRequestId::new("req_fixture").expect("fixture request id"),
                ),
                gateway_route: None,
            })
        }

        fn classify_http(
            &self,
            _status: u16,
            _headers: &HeaderView<'_>,
            _body: &BoundedBody,
        ) -> ProviderFailure {
            ProviderFailure::new(RedactedDetail::internal(
                ProviderFailureKind::ServerError,
                "fixture",
            ))
        }

        fn rate_limit_feedback(&self, _headers: &HeaderView<'_>) -> RateLimitFeedback {
            RateLimitFeedback::none()
        }

        fn request_id(
            &self,
            _headers: &HeaderView<'_>,
            state: &DialectState,
        ) -> Option<ProviderRequestId> {
            state
                .response_started
                .then(|| ProviderRequestId::new("req_fixture").expect("fixture request id"))
        }
    }

    #[derive(Debug, Default)]
    struct CountingStartSink(AtomicUsize);

    impl ResponseStartSink for CountingStartSink {
        fn mark(
            &self,
            provider_request_id: Option<ProviderRequestId>,
        ) -> BoxFuture<'_, Result<(), ResponseStartSinkError>> {
            assert_eq!(
                provider_request_id.as_ref().map(ProviderRequestId::as_str),
                Some("req_fixture")
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Debug, Clone)]
    struct CancellingStartSink(CancelToken);

    impl ResponseStartSink for CancellingStartSink {
        fn mark(
            &self,
            _provider_request_id: Option<ProviderRequestId>,
        ) -> BoxFuture<'_, Result<(), ResponseStartSinkError>> {
            self.0.cancel();
            Box::pin(async { Ok(()) })
        }
    }

    fn model() -> QualifiedModel {
        fixture::qualified_entry(
            ProviderId::Openai,
            "fixture-model",
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
            ]),
        )
    }

    fn headers() -> reqwest::header::HeaderMap {
        reqwest::header::HeaderMap::new()
    }

    async fn consume<S>(
        body: S,
        budget: &StreamBudget,
    ) -> Result<ConsumedStream, super::StreamConsumeError>
    where
        S: futures::Stream<Item = Result<Bytes, ()>> + Send,
    {
        let headers = headers();
        consume_provider_stream(
            &FixtureAdapter,
            &model(),
            body,
            HeaderView::new(&headers),
            budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await
    }

    #[tokio::test]
    async fn fragmented_stream_uses_one_decoder_and_marks_response_start_once() {
        let sink = CountingStartSink::default();
        let headers = headers();
        let result = consume_provider_stream(
            &FixtureAdapter,
            &model(),
            stream::iter([
                Ok::<_, ()>(Bytes::from_static(b"data: sta")),
                Ok(Bytes::from_static(b"rt\n\ndata: done\n\n")),
            ]),
            HeaderView::new(&headers),
            &StreamBudget::default(),
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &sink,
        )
        .await
        .expect("stream completes");
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
        assert_eq!(result.frames, 2);
        assert_eq!(result.response_bytes, 25);
        assert!(result.first_frame_after.is_some());
        assert_eq!(result.response.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn a_drop_before_the_first_frame_is_possibly_sent() {
        let error = consume(
            stream::iter([Err::<Bytes, ()>(())]),
            &StreamBudget::default(),
        )
        .await
        .expect_err("transport drop");
        assert_eq!(error.proof(), DispatchProof::PossiblySent);
        assert_eq!(error.kind(), ProviderFailureKind::Transport);
        assert!(!error.response_started);
    }

    #[tokio::test]
    async fn a_drop_after_the_first_frame_preserves_response_started() {
        let error = consume(
            stream::iter([
                Ok(Bytes::from_static(b"data: start\n\n")),
                Err::<Bytes, ()>(()),
            ]),
            &StreamBudget::default(),
        )
        .await
        .expect_err("transport drop");
        assert_eq!(error.proof(), DispatchProof::ResponseStarted);
        assert_eq!(error.kind(), ProviderFailureKind::Transport);
        assert_eq!(
            error
                .provider_request_id
                .as_ref()
                .map(ProviderRequestId::as_str),
            Some("req_fixture")
        );
    }

    #[tokio::test]
    async fn cancellation_after_the_first_frame_stops_within_the_same_chunk() {
        let cancel = CancelToken::new();
        let sink = CancellingStartSink(cancel.clone());
        let headers = headers();
        let chunk = Bytes::from_static(b"data: start\n\ndata: done\n\n");
        let error = consume_provider_stream(
            &FixtureAdapter,
            &model(),
            stream::iter([Ok::<_, ()>(chunk.clone())]),
            HeaderView::new(&headers),
            &StreamBudget::default(),
            &NullPreviewSink,
            &cancel,
            Instant::now(),
            &sink,
        )
        .await
        .expect_err("response-start cancellation settles immediately");
        assert!(matches!(error.failure, StreamFailure::Cancelled));
        assert_eq!(error.kind(), ProviderFailureKind::Cancelled);
        assert_eq!(error.proof(), DispatchProof::ResponseStarted);
        assert_eq!(error.response_bytes, chunk.len() as u64);
        assert_eq!(error.frames, 1);
        assert_eq!(error.frames_after_cancel, 0);
    }

    #[tokio::test]
    async fn the_staged_deepseek_seam_uses_the_production_dialect() {
        let entry = fixture::entry(
            ProviderId::Deepseek,
            "deepseek-qualification-fixture",
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
            ]),
        );
        let headers = headers();
        let result = consume_deepseek_qualification_stream(
            &entry,
            stream::iter([Ok::<_, ()>(Bytes::from_static(
                concat!(
                    "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,",
                    "\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],",
                    "\"usage\":null}\n\n",
                    "data: [DONE]\n\n"
                )
                .as_bytes(),
            ))]),
            HeaderView::new(&headers),
            &StreamBudget::default(),
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await
        .expect("the staged DeepSeek stream uses the production decoder");
        assert_eq!(result.frames, 2);
        assert_eq!(result.response.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn the_qualification_seam_refuses_an_active_entry() {
        let mut entry = fixture::entry(
            ProviderId::Deepseek,
            "deepseek-qualification-fixture",
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
            ]),
        );
        entry.state = EntryState::Active;
        let headers = headers();
        let error = consume_deepseek_qualification_stream(
            &entry,
            stream::empty::<Result<Bytes, ()>>(),
            HeaderView::new(&headers),
            &StreamBudget::default(),
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await
        .expect_err("active entries must use the ordinary qualified-model path");
        assert_eq!(error, QualificationStreamError::Candidate);
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_first_frame_is_a_typed_timeout() {
        let budget = StreamBudget {
            first_frame_timeout: Duration::from_millis(5),
            total_deadline: Duration::from_secs(1),
            ..StreamBudget::default()
        };
        let error = consume(stream::pending(), &budget)
            .await
            .expect_err("first frame timeout");
        assert_eq!(error.proof(), DispatchProof::PossiblySent);
        assert_eq!(error.kind(), ProviderFailureKind::Timeout);
        assert!(matches!(error.failure, StreamFailure::Budget(_)));
    }

    #[tokio::test]
    async fn an_injected_oversized_frame_is_a_typed_protocol_violation() {
        let budget = StreamBudget {
            max_frame_bytes: 8,
            ..StreamBudget::default()
        };
        let error = consume(
            stream::iter([Ok(Bytes::from_static(b"data: too-long\n\n"))]),
            &budget,
        )
        .await
        .expect_err("oversized frame");
        assert_eq!(error.proof(), DispatchProof::PossiblySent);
        assert_eq!(error.kind(), ProviderFailureKind::ProtocolViolation);
    }

    #[tokio::test]
    async fn bytes_after_a_terminal_frame_are_not_consumed() {
        let result = consume(
            stream::iter([
                Ok(Bytes::from_static(b"data: done\n\n")),
                Err::<Bytes, ()>(()),
            ]),
            &StreamBudget::default(),
        )
        .await
        .expect("terminal frame is authoritative");
        assert_eq!(result.frames, 1);
        assert_eq!(result.response.stop_reason, StopReason::EndTurn);
    }
}
