//! Typed evidence and fail-closed verification for the P-01--P-23 matrix.
//!
//! A protected driver may observe provider bytes, but this boundary accepts
//! only closed enums, hashes, counters and booleans. It therefore cannot copy
//! an unredacted provider diagnostic into a receipt fact. A driver failure uses
//! `RedactedDetail` through the executor instead.

use aex_brain_provider_gateway::DispatchProof;
use aex_brain_provider_gateway::error::{ProviderFailureKind, RateLimitSource};
use aex_model_catalog::canonical::StopReason;
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::receipt::{ObservedFact, ProbeId, ProbeOutcome};
use aex_wire::ContentHash;

use crate::ProbeRun;
use crate::executor::QualificationTarget;

/// Closed error-body shapes which P-20 may retain as receipt metadata.
///
/// The evidence bundle may retain a separately redacted sample. The receipt
/// records only this shape token, never provider response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorBodyShape {
    /// `{"error":{"message":...,"type"|"code":...}}`.
    OpenAiEnvelope,
    /// `{"type":"error","error":{...}}`.
    AnthropicEnvelope,
    /// Google's documented snake-case error envelope.
    GoogleSnakeCase,
    /// Google's classic gRPC-style error envelope.
    GoogleRpc,
    /// The provider supplied a status without a structured error body.
    StatusOnly,
    /// A bounded, redacted sample exists but matches no known closed shape.
    OtherRedacted,
}

impl ErrorBodyShape {
    const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiEnvelope => "openai_envelope",
            Self::AnthropicEnvelope => "anthropic_envelope",
            Self::GoogleSnakeCase => "google_snake_case",
            Self::GoogleRpc => "google_rpc",
            Self::StatusOnly => "status_only",
            Self::OtherRedacted => "other_redacted",
        }
    }
}

/// Provider-independent observations from one configured probe program.
///
/// There is one variant per registry probe. The executor rejects evidence for
/// another probe instead of relabelling it, and [`verify`] is the only place a
/// configured driver can turn observations into a passing row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvidence {
    /// P-01: minimal text and streamed/non-streamed canonical shape parity.
    MinimalText {
        /// Canonical streamed response shape.
        streamed_shape: ContentHash,
        /// Canonical non-streamed response shape.
        non_streamed_shape: ContentHash,
        /// Decoded text bytes; zero is not a minimal text response.
        text_bytes: u64,
    },
    /// P-02: long, ordered, lossless streaming.
    LongStream {
        /// Decoded output bytes.
        output_bytes: u64,
        /// Provider dialect frames decoded.
        frames: u32,
        /// Whether provider order was preserved.
        ordered: bool,
        /// Whether reassembly found no missing/coalesced content.
        lossless: bool,
    },
    /// P-03: system instruction compliance.
    SystemInstruction {
        /// Whether the configured deterministic oracle matched.
        honoured: bool,
    },
    /// P-04: one schema-valid tool call.
    SingleToolCall {
        /// Calls decoded.
        calls: u16,
        /// Whether the decoded arguments validate against the configured schema.
        arguments_valid: bool,
    },
    /// P-05: parallel calls and distinct provider ids.
    ParallelToolCalls {
        /// Calls decoded.
        calls: u16,
        /// Whether every call id was distinct.
        distinct_ids: bool,
    },
    /// P-06: tool-result round trip.
    ToolResultRoundTrip {
        /// Whether the second assistant turn sealed successfully.
        second_turn_completed: bool,
    },
    /// P-07: every tool-choice mode declared by the target.
    ToolChoiceModes {
        /// Number of declared modes exercised.
        exercised: u8,
        /// Number whose configured oracle passed.
        passed: u8,
    },
    /// P-08: structured output.
    StructuredOutput {
        /// Whether the decoded result validates against the configured schema.
        schema_valid: bool,
    },
    /// P-09: reasoning content and accounting.
    Reasoning {
        /// Whether reasoning content was decoded.
        content_observed: bool,
        /// Normalized reasoning-token count.
        reasoning_tokens: u64,
    },
    /// P-10: reasoning replay positive and negative arms.
    ReasoningReplay {
        /// Replaying the observed token/material was accepted.
        replay_accepted: bool,
        /// Omitting required material was rejected.
        omission_rejected: bool,
    },
    /// P-11: complete usage in stream and non-stream forms.
    Usage {
        /// Streamed usage was complete.
        stream_complete: bool,
        /// Non-streamed usage was complete.
        non_stream_complete: bool,
        /// Both forms match the catalog entry's declared mapping.
        mapping_matches: bool,
    },
    /// P-12: prompt-cache accounting.
    PromptCache {
        /// Cache-read input tokens on the repeated prefix.
        cache_read_tokens: u64,
    },
    /// P-13: output ceiling stop mapping.
    OutputCeiling {
        /// Canonical stop reason.
        stop_reason: StopReason,
    },
    /// P-14: stop-sequence mapping.
    StopSequence {
        /// Canonical stop reason.
        stop_reason: StopReason,
    },
    /// P-15: cancellation after provider bytes were observed.
    Cancellation {
        /// Typed failure kind.
        kind: ProviderFailureKind,
        /// Response bytes recorded before cancellation.
        response_bytes: u64,
        /// Frames consumed after cancellation was observed.
        frames_after_cancel: u32,
    },
    /// P-16: long context at or above eighty percent.
    LongContext {
        /// Prompt tokens sent, measured by the configured tokenizer/oracle.
        prompt_tokens: u32,
        /// Exact context window from the owner-supplied catalog entry.
        context_window_tokens: u32,
        /// Whether the response completed.
        completed: bool,
    },
    /// P-17: context window plus one.
    ContextOverflow {
        /// Typed failure kind.
        kind: ProviderFailureKind,
        /// Whether the provider silently truncated instead of rejecting.
        truncated: bool,
    },
    /// P-18: injected drops around response-head/frame/terminal boundaries.
    ForcedDrops {
        /// Drop after send and before response headers.
        pre_headers: DispatchProof,
        /// Drop after response headers and before a validated frame.
        post_headers_pre_frame: DispatchProof,
        /// Drop after a validated frame, including a mid-frame tail.
        mid_frame: DispatchProof,
        /// A terminal frame remained authoritative when the close was dropped.
        post_terminal_completed: bool,
    },
    /// P-19: rate-limit feedback or positive absence.
    RateLimit {
        /// Adapter-normalized feedback source.
        source: RateLimitSource,
        /// Whether at least one bounded feedback field was observed.
        feedback_observed: bool,
        /// Whether `NotProvided` was recorded as a positive answer.
        absence_recorded: bool,
    },
    /// P-20: three required error-taxonomy arms.
    ErrorTaxonomy {
        /// Wrong-key classification.
        wrong_key: ProviderFailureKind,
        /// Malformed-body classification.
        malformed_body: ProviderFailureKind,
        /// Unknown-model classification.
        unknown_model: ProviderFailureKind,
        /// Closed body shape for each arm, in the same order.
        body_shapes: [ErrorBodyShape; 3],
    },
    /// P-21: credential-leak scan over response-derived artefacts.
    CredentialLeak {
        /// Number of bounded artefacts examined.
        artifacts_scanned: u32,
        /// Exact or encoded credential matches found.
        matches: u32,
    },
    /// P-22: oversized-frame injection.
    OversizedFrame {
        /// Typed failure kind.
        kind: ProviderFailureKind,
        /// Dispatch proof at rejection.
        proof: DispatchProof,
    },
    /// P-23: idle-stream injection.
    IdleTimeout {
        /// Typed failure kind.
        kind: ProviderFailureKind,
        /// Configured idle limit.
        idle_limit_ms: u32,
        /// Time at which the typed result was observed.
        observed_after_ms: u32,
        /// Whether the driver outlived its enclosing total deadline.
        hung: bool,
    },
}

impl ProbeEvidence {
    /// Probe identity structurally carried by this evidence variant.
    #[must_use]
    pub const fn probe(&self) -> ProbeId {
        match self {
            Self::MinimalText { .. } => ProbeId::P01,
            Self::LongStream { .. } => ProbeId::P02,
            Self::SystemInstruction { .. } => ProbeId::P03,
            Self::SingleToolCall { .. } => ProbeId::P04,
            Self::ParallelToolCalls { .. } => ProbeId::P05,
            Self::ToolResultRoundTrip { .. } => ProbeId::P06,
            Self::ToolChoiceModes { .. } => ProbeId::P07,
            Self::StructuredOutput { .. } => ProbeId::P08,
            Self::Reasoning { .. } => ProbeId::P09,
            Self::ReasoningReplay { .. } => ProbeId::P10,
            Self::Usage { .. } => ProbeId::P11,
            Self::PromptCache { .. } => ProbeId::P12,
            Self::OutputCeiling { .. } => ProbeId::P13,
            Self::StopSequence { .. } => ProbeId::P14,
            Self::Cancellation { .. } => ProbeId::P15,
            Self::LongContext { .. } => ProbeId::P16,
            Self::ContextOverflow { .. } => ProbeId::P17,
            Self::ForcedDrops { .. } => ProbeId::P18,
            Self::RateLimit { .. } => ProbeId::P19,
            Self::ErrorTaxonomy { .. } => ProbeId::P20,
            Self::CredentialLeak { .. } => ProbeId::P21,
            Self::OversizedFrame { .. } => ProbeId::P22,
            Self::IdleTimeout { .. } => ProbeId::P23,
        }
    }
}

/// Verifies one typed observation and creates its receipt row.
///
/// Failure details and observed facts are generated only from static labels,
/// closed enums and counters. Provider diagnostics cannot enter this function.
#[must_use]
pub(crate) fn verify(
    target: &QualificationTarget,
    evidence: &ProbeEvidence,
    duration_ms: u32,
) -> ProbeRun {
    let probe = evidence.probe();
    let (passed, detail, observed) = evaluate(target, evidence);
    ProbeRun {
        probe,
        outcome: if passed {
            ProbeOutcome::Pass
        } else {
            ProbeOutcome::Fail {
                detail: BoundedString::truncating(detail),
            }
        },
        observed,
        duration_ms,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive match is the auditable P-01--P-23 qualification rule"
)]
fn evaluate(
    target: &QualificationTarget,
    evidence: &ProbeEvidence,
) -> (bool, &'static str, Vec<ObservedFact>) {
    match evidence {
        ProbeEvidence::MinimalText {
            streamed_shape,
            non_streamed_shape,
            text_bytes,
        } => (
            text_bytes > &0 && streamed_shape == non_streamed_shape,
            "minimal text was empty or stream/non-stream shapes differed",
            vec![counter("text_bytes", *text_bytes)],
        ),
        ProbeEvidence::LongStream {
            output_bytes,
            frames,
            ordered,
            lossless,
        } => (
            output_bytes >= &8_192 && frames >= &200 && *ordered && *lossless,
            "long stream did not preserve at least 8 KiB over 200 ordered frames",
            vec![
                counter("output_bytes", *output_bytes),
                counter("frames", u64::from(*frames)),
            ],
        ),
        ProbeEvidence::SystemInstruction { honoured } => (
            *honoured,
            "system instruction oracle did not match",
            Vec::new(),
        ),
        ProbeEvidence::SingleToolCall {
            calls,
            arguments_valid,
        } => (
            *calls == 1 && *arguments_valid,
            "single tool call count or argument schema did not match",
            vec![counter("tool_calls", u64::from(*calls))],
        ),
        ProbeEvidence::ParallelToolCalls {
            calls,
            distinct_ids,
        } => (
            *calls >= 2 && *distinct_ids,
            "parallel tool calls were absent or reused an id",
            vec![counter("tool_calls", u64::from(*calls))],
        ),
        ProbeEvidence::ToolResultRoundTrip {
            second_turn_completed,
        } => (
            *second_turn_completed,
            "tool-result round trip did not produce a second assistant turn",
            Vec::new(),
        ),
        ProbeEvidence::ToolChoiceModes { exercised, passed } => {
            let expected = u8::try_from(
                [
                    aex_model_catalog::document::Capability::ToolChoiceRequired,
                    aex_model_catalog::document::Capability::ToolChoiceNamed,
                    aex_model_catalog::document::Capability::ToolChoiceNone,
                ]
                .into_iter()
                .filter(|capability| target.capabilities.has(*capability))
                .count(),
            )
            .unwrap_or(u8::MAX);
            (
                expected > 0 && *exercised == expected && *passed == expected,
                "not every declared tool-choice mode behaved as configured",
                vec![counter("tool_choice_modes", u64::from(*exercised))],
            )
        }
        ProbeEvidence::StructuredOutput { schema_valid } => (
            *schema_valid,
            "structured output did not validate against the configured schema",
            Vec::new(),
        ),
        ProbeEvidence::Reasoning {
            content_observed,
            reasoning_tokens,
        } => (
            *content_observed && reasoning_tokens > &0,
            "reasoning content or reasoning-token accounting was absent",
            vec![counter("reasoning_tokens", *reasoning_tokens)],
        ),
        ProbeEvidence::ReasoningReplay {
            replay_accepted,
            omission_rejected,
        } => (
            *replay_accepted && *omission_rejected,
            "reasoning replay positive or omission-negative arm did not match",
            Vec::new(),
        ),
        ProbeEvidence::Usage {
            stream_complete,
            non_stream_complete,
            mapping_matches,
        } => (
            *stream_complete && *non_stream_complete && *mapping_matches,
            "stream/non-stream usage was incomplete or mapping did not match",
            Vec::new(),
        ),
        ProbeEvidence::PromptCache { cache_read_tokens } => (
            cache_read_tokens > &0,
            "repeated prefix produced no cache-read tokens",
            vec![counter("cache_read_tokens", *cache_read_tokens)],
        ),
        ProbeEvidence::OutputCeiling { stop_reason } => (
            *stop_reason == StopReason::MaxOutputTokens,
            "output ceiling did not map to MaxOutputTokens",
            Vec::new(),
        ),
        ProbeEvidence::StopSequence { stop_reason } => (
            *stop_reason == StopReason::StopSequence,
            "stop sequence did not map to StopSequence",
            Vec::new(),
        ),
        ProbeEvidence::Cancellation {
            kind,
            response_bytes,
            frames_after_cancel,
        } => (
            *kind == ProviderFailureKind::Cancelled
                && response_bytes > &0
                && *frames_after_cancel == 0,
            "cancellation was not typed, byte-accounted, and terminal for consumption",
            vec![counter("response_bytes", *response_bytes)],
        ),
        ProbeEvidence::LongContext {
            prompt_tokens,
            context_window_tokens,
            completed,
        } => (
            *completed
                && *context_window_tokens > 0
                && u64::from(*prompt_tokens) * 100 >= u64::from(*context_window_tokens) * 80,
            "long context was below eighty percent or did not complete",
            vec![counter("prompt_tokens", u64::from(*prompt_tokens))],
        ),
        ProbeEvidence::ContextOverflow { kind, truncated } => (
            *kind == ProviderFailureKind::ContextOverflow && !*truncated,
            "context window plus one was not rejected as ContextOverflow",
            Vec::new(),
        ),
        ProbeEvidence::ForcedDrops {
            pre_headers,
            post_headers_pre_frame,
            mid_frame,
            post_terminal_completed,
        } => (
            *pre_headers == DispatchProof::PossiblySent
                && *post_headers_pre_frame == DispatchProof::PossiblySent
                && *mid_frame == DispatchProof::ResponseStarted
                && *post_terminal_completed,
            "forced-drop dispatch proofs did not match the send/frame boundaries",
            Vec::new(),
        ),
        ProbeEvidence::RateLimit {
            source,
            feedback_observed,
            absence_recorded,
        } => {
            let passed = match source {
                RateLimitSource::NotProvided => *absence_recorded && !*feedback_observed,
                _ => *feedback_observed,
            };
            (
                passed,
                "rate-limit feedback was neither observed nor positively absent",
                vec![fact("rate_limit_source", rate_limit_source(*source))],
            )
        }
        ProbeEvidence::ErrorTaxonomy {
            wrong_key,
            malformed_body,
            unknown_model,
            body_shapes,
        } => (
            *wrong_key == ProviderFailureKind::Authentication
                && *malformed_body == ProviderFailureKind::InvalidRequest
                && *unknown_model == ProviderFailureKind::ModelNotFound,
            "wrong-key, malformed-body, or unknown-model taxonomy differed",
            vec![fact(
                "error_body_shapes",
                &format!(
                    "{},{},{}",
                    body_shapes[0].as_str(),
                    body_shapes[1].as_str(),
                    body_shapes[2].as_str()
                ),
            )],
        ),
        ProbeEvidence::CredentialLeak {
            artifacts_scanned,
            matches,
        } => (
            *artifacts_scanned > 0 && *matches == 0,
            "credential scan was empty or found response-derived matches",
            vec![counter("artifacts_scanned", u64::from(*artifacts_scanned))],
        ),
        ProbeEvidence::OversizedFrame { kind, proof } => (
            *kind == ProviderFailureKind::ProtocolViolation
                && *proof == DispatchProof::PossiblySent,
            "oversized frame was not a pre-frame typed protocol violation",
            Vec::new(),
        ),
        ProbeEvidence::IdleTimeout {
            kind,
            idle_limit_ms,
            observed_after_ms,
            hung,
        } => (
            *kind == ProviderFailureKind::Timeout
                && *idle_limit_ms > 0
                && observed_after_ms >= idle_limit_ms
                && !*hung,
            "idle stream did not settle as a bounded timeout",
            vec![counter("idle_limit_ms", u64::from(*idle_limit_ms))],
        ),
    }
}

fn counter(key: &'static str, value: u64) -> ObservedFact {
    fact(key, &value.to_string())
}

fn fact(key: &'static str, value: &str) -> ObservedFact {
    ObservedFact {
        key: BoundedString::truncating(key),
        value: BoundedString::truncating(value),
    }
}

const fn rate_limit_source(source: RateLimitSource) -> &'static str {
    match source {
        RateLimitSource::NotProvided => "not_provided",
        RateLimitSource::RetryAfterHeader => "retry_after_header",
        RateLimitSource::VendorHeaders => "vendor_headers",
        RateLimitSource::ErrorBody => "error_body",
    }
}

#[cfg(test)]
mod tests {
    use aex_brain_provider_gateway::DispatchProof;
    use aex_brain_provider_gateway::error::{ProviderFailureKind, RateLimitSource};
    use aex_model_catalog::canonical::StopReason;
    use aex_model_catalog::receipt::{ProbeId, ProbeOutcome};
    use aex_wire::ContentHash;
    use aex_wire::provider::ProviderId;

    use super::{ErrorBodyShape, ProbeEvidence, verify};
    use crate::executor::{QualificationTarget, profile};

    fn target() -> QualificationTarget {
        QualificationTarget::new(
            ProviderId::Anthropic,
            "owner-supplied-model",
            profile(ProviderId::Anthropic).capability_ceiling,
        )
        .expect("the target is syntactically valid test input, not a catalog entry")
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive positive table deliberately has one row per registry probe"
    )]
    fn passing() -> Vec<ProbeEvidence> {
        let shape = ContentHash::of(b"same canonical shape");
        vec![
            ProbeEvidence::MinimalText {
                streamed_shape: shape,
                non_streamed_shape: shape,
                text_bytes: 2,
            },
            ProbeEvidence::LongStream {
                output_bytes: 8_192,
                frames: 200,
                ordered: true,
                lossless: true,
            },
            ProbeEvidence::SystemInstruction { honoured: true },
            ProbeEvidence::SingleToolCall {
                calls: 1,
                arguments_valid: true,
            },
            ProbeEvidence::ParallelToolCalls {
                calls: 2,
                distinct_ids: true,
            },
            ProbeEvidence::ToolResultRoundTrip {
                second_turn_completed: true,
            },
            ProbeEvidence::ToolChoiceModes {
                exercised: 3,
                passed: 3,
            },
            ProbeEvidence::StructuredOutput { schema_valid: true },
            ProbeEvidence::Reasoning {
                content_observed: true,
                reasoning_tokens: 1,
            },
            ProbeEvidence::ReasoningReplay {
                replay_accepted: true,
                omission_rejected: true,
            },
            ProbeEvidence::Usage {
                stream_complete: true,
                non_stream_complete: true,
                mapping_matches: true,
            },
            ProbeEvidence::PromptCache {
                cache_read_tokens: 1,
            },
            ProbeEvidence::OutputCeiling {
                stop_reason: StopReason::MaxOutputTokens,
            },
            ProbeEvidence::StopSequence {
                stop_reason: StopReason::StopSequence,
            },
            ProbeEvidence::Cancellation {
                kind: ProviderFailureKind::Cancelled,
                response_bytes: 1,
                frames_after_cancel: 0,
            },
            ProbeEvidence::LongContext {
                prompt_tokens: 80,
                context_window_tokens: 100,
                completed: true,
            },
            ProbeEvidence::ContextOverflow {
                kind: ProviderFailureKind::ContextOverflow,
                truncated: false,
            },
            ProbeEvidence::ForcedDrops {
                pre_headers: DispatchProof::PossiblySent,
                post_headers_pre_frame: DispatchProof::PossiblySent,
                mid_frame: DispatchProof::ResponseStarted,
                post_terminal_completed: true,
            },
            ProbeEvidence::RateLimit {
                source: RateLimitSource::NotProvided,
                feedback_observed: false,
                absence_recorded: true,
            },
            ProbeEvidence::ErrorTaxonomy {
                wrong_key: ProviderFailureKind::Authentication,
                malformed_body: ProviderFailureKind::InvalidRequest,
                unknown_model: ProviderFailureKind::ModelNotFound,
                body_shapes: [
                    ErrorBodyShape::AnthropicEnvelope,
                    ErrorBodyShape::AnthropicEnvelope,
                    ErrorBodyShape::AnthropicEnvelope,
                ],
            },
            ProbeEvidence::CredentialLeak {
                artifacts_scanned: 1,
                matches: 0,
            },
            ProbeEvidence::OversizedFrame {
                kind: ProviderFailureKind::ProtocolViolation,
                proof: DispatchProof::PossiblySent,
            },
            ProbeEvidence::IdleTimeout {
                kind: ProviderFailureKind::Timeout,
                idle_limit_ms: 10,
                observed_after_ms: 10,
                hung: false,
            },
        ]
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive negative table deliberately has one row per registry probe"
    )]
    fn failing() -> Vec<ProbeEvidence> {
        let left = ContentHash::of(b"left");
        let right = ContentHash::of(b"right");
        vec![
            ProbeEvidence::MinimalText {
                streamed_shape: left,
                non_streamed_shape: right,
                text_bytes: 1,
            },
            ProbeEvidence::LongStream {
                output_bytes: 8_191,
                frames: 200,
                ordered: true,
                lossless: true,
            },
            ProbeEvidence::SystemInstruction { honoured: false },
            ProbeEvidence::SingleToolCall {
                calls: 2,
                arguments_valid: true,
            },
            ProbeEvidence::ParallelToolCalls {
                calls: 2,
                distinct_ids: false,
            },
            ProbeEvidence::ToolResultRoundTrip {
                second_turn_completed: false,
            },
            ProbeEvidence::ToolChoiceModes {
                exercised: 2,
                passed: 2,
            },
            ProbeEvidence::StructuredOutput {
                schema_valid: false,
            },
            ProbeEvidence::Reasoning {
                content_observed: true,
                reasoning_tokens: 0,
            },
            ProbeEvidence::ReasoningReplay {
                replay_accepted: true,
                omission_rejected: false,
            },
            ProbeEvidence::Usage {
                stream_complete: true,
                non_stream_complete: false,
                mapping_matches: true,
            },
            ProbeEvidence::PromptCache {
                cache_read_tokens: 0,
            },
            ProbeEvidence::OutputCeiling {
                stop_reason: StopReason::EndTurn,
            },
            ProbeEvidence::StopSequence {
                stop_reason: StopReason::EndTurn,
            },
            ProbeEvidence::Cancellation {
                kind: ProviderFailureKind::Cancelled,
                response_bytes: 1,
                frames_after_cancel: 1,
            },
            ProbeEvidence::LongContext {
                prompt_tokens: 79,
                context_window_tokens: 100,
                completed: true,
            },
            ProbeEvidence::ContextOverflow {
                kind: ProviderFailureKind::InvalidRequest,
                truncated: false,
            },
            ProbeEvidence::ForcedDrops {
                pre_headers: DispatchProof::NotSent,
                post_headers_pre_frame: DispatchProof::PossiblySent,
                mid_frame: DispatchProof::ResponseStarted,
                post_terminal_completed: true,
            },
            ProbeEvidence::RateLimit {
                source: RateLimitSource::NotProvided,
                feedback_observed: false,
                absence_recorded: false,
            },
            ProbeEvidence::ErrorTaxonomy {
                wrong_key: ProviderFailureKind::Authentication,
                malformed_body: ProviderFailureKind::InvalidRequest,
                unknown_model: ProviderFailureKind::InvalidRequest,
                body_shapes: [
                    ErrorBodyShape::OtherRedacted,
                    ErrorBodyShape::OtherRedacted,
                    ErrorBodyShape::OtherRedacted,
                ],
            },
            ProbeEvidence::CredentialLeak {
                artifacts_scanned: 1,
                matches: 1,
            },
            ProbeEvidence::OversizedFrame {
                kind: ProviderFailureKind::ProtocolViolation,
                proof: DispatchProof::ResponseStarted,
            },
            ProbeEvidence::IdleTimeout {
                kind: ProviderFailureKind::Timeout,
                idle_limit_ms: 10,
                observed_after_ms: 9,
                hung: false,
            },
        ]
    }

    #[test]
    fn every_probe_has_a_positive_typed_evidence_path() {
        let target = target();
        let evidence = passing();
        assert_eq!(
            evidence
                .iter()
                .map(ProbeEvidence::probe)
                .collect::<Vec<_>>(),
            ProbeId::ALL
        );
        for item in evidence {
            let probe = item.probe();
            let run = verify(&target, &item, 7);
            assert_eq!(run.probe, probe);
            assert_eq!(run.outcome, ProbeOutcome::Pass, "{probe:?}");
            assert_eq!(run.duration_ms, 7);
        }
    }

    #[test]
    fn every_probe_has_a_fail_closed_regression() {
        let target = target();
        let evidence = failing();
        assert_eq!(
            evidence
                .iter()
                .map(ProbeEvidence::probe)
                .collect::<Vec<_>>(),
            ProbeId::ALL
        );
        for item in evidence {
            let probe = item.probe();
            let run = verify(&target, &item, 0);
            assert!(
                matches!(run.outcome, ProbeOutcome::Fail { .. }),
                "{probe:?} unexpectedly passed"
            );
        }
    }

    #[test]
    fn receipt_facts_are_closed_tokens_and_counters_not_provider_text() {
        let target = target();
        let rate_limit = verify(
            &target,
            &ProbeEvidence::RateLimit {
                source: RateLimitSource::ErrorBody,
                feedback_observed: true,
                absence_recorded: false,
            },
            1,
        );
        assert_eq!(rate_limit.observed[0].value.as_str(), "error_body");

        let taxonomy = verify(
            &target,
            &ProbeEvidence::ErrorTaxonomy {
                wrong_key: ProviderFailureKind::Authentication,
                malformed_body: ProviderFailureKind::InvalidRequest,
                unknown_model: ProviderFailureKind::ModelNotFound,
                body_shapes: [
                    ErrorBodyShape::OpenAiEnvelope,
                    ErrorBodyShape::GoogleSnakeCase,
                    ErrorBodyShape::GoogleRpc,
                ],
            },
            1,
        );
        assert_eq!(
            taxonomy.observed[0].value.as_str(),
            "openai_envelope,google_snake_case,google_rpc"
        );
    }
}
