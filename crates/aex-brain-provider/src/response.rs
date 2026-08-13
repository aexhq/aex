//! rig stream → canonical blocks, usage, stop reason and preview frames.
//!
//! The one translation site back from rig. Two accepted simplifications,
//! recorded deliberately:
//!
//! - **stop granularity**: rig's public streaming surface exposes the
//!   aggregated content but not the provider `finish_reason`. A turn that
//!   ends with one or more tool calls seals `ToolUse`; everything else seals
//!   `EndTurn`. `MaxOutputTokens`/`StopSequence`/`Refusal` are not
//!   distinguishable through rig and collapse into `EndTurn`.
//! - **request ids and byte counts**: rig does not publish provider request
//!   ids or byte counts generically, so the receipt records none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aex_brain_app::ports::proof::PreviewSink;
use aex_model_catalog::canonical::{
    CanonicalBlock, NormalizedUsage, PreviewFrame, ReasoningBlock, ReasoningBody, ReasoningToken,
    StopReason, UsageCompleteness,
};
use aex_model_catalog::primitives::{BoundedString, ToolCallId};
use aex_model_vocabulary::DialectClass;
use aex_wire::provider::ProviderId;
use aex_wire::{CanonicalJson, ResourceName, to_jcs_bytes};
use futures::StreamExt as _;
use rig_core::completion::message::ReasoningContent;
use rig_core::completion::{CompletionModel, Usage};
use rig_core::streaming::{StreamedAssistantContent, ToolCallDeltaContent};

/// What one rig stream produced, before sealing.
pub(crate) struct StreamOutcome {
    /// The assembled canonical blocks, in order.
    pub blocks: Vec<CanonicalBlock>,
    /// Provider usage, normalized.
    pub usage: NormalizedUsage,
    /// Why the model stopped.
    pub stop: StopReason,
    /// When the first decoded item arrived, for the receipt.
    pub first_item_at: Option<Instant>,
}

/// Why one stream attempt did not produce an outcome.
pub(crate) enum StreamFailure {
    /// A definitive non-2xx rejection (429/503). Never sent past the provider
    /// rejection; safe to re-send.
    Definitive {
        /// The HTTP status.
        status: u16,
        /// The redacted body, where one arrived.
        detail: String,
    },
    /// A transport drop before or during the stream.
    Transport {
        /// The redacted detail.
        detail: String,
    },
    /// The caller cancelled the dispatch mid-stream.
    Cancelled,
    /// The stream ended but its content could not become a canonical message.
    Protocol {
        /// The redacted detail.
        detail: String,
    },
    /// The durable response-started write failed.
    EffectStore {
        /// The redacted detail.
        detail: String,
    },
}

/// Whether a failure is a definitive non-2xx rejection the retry loop may
/// re-send after.
pub(crate) const fn is_retryable(status: u16) -> bool {
    status == 429 || status == 503
}

/// Drives one stream to completion, folding items into canonical blocks.
///
/// The first `Ok` item fires `on_started` (the durable response-started
/// write); every item after that counts toward the outcome. An error item
/// terminates the stream (rig guarantees one error item then the stream ends).
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one stream fold: item classification, preview emission and block assembly stay together"
)]
pub(crate) async fn consume_stream<M, F, Fut>(
    model: &M,
    request: &rig_core::completion::CompletionRequest,
    _dialect: DialectClass,
    provider: ProviderId,
    preview: &dyn PreviewSink,
    cancel: &aex_brain_app::ports::proof::CancelToken,
    response_started: &AtomicBool,
    mut on_started: F,
) -> Result<StreamOutcome, StreamFailure>
where
    M: CompletionModel,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), StreamFailure>>,
{
    let mut stream = model
        .stream(request.clone())
        .await
        .map_err(|error| classify_stream_error(&error))?;

    let mut blocks: Vec<CanonicalBlock> = Vec::new();
    let mut text: Option<String> = None;
    let mut reasoning: Option<(String, Option<String>, bool)> = None; // (text, signature, redacted)
    let mut first_item_at: Option<Instant> = None;

    let flush_text = |blocks: &mut Vec<CanonicalBlock>, text: &mut Option<String>| {
        if let Some(text) = text.take()
            && !text.is_empty()
        {
            blocks.push(CanonicalBlock::Text {
                text: BoundedString::truncating(&text),
                annotations: Vec::new(),
            });
        }
    };
    let flush_reasoning =
        |blocks: &mut Vec<CanonicalBlock>, pending: &mut Option<(String, Option<String>, bool)>| {
            if let Some((text, signature, redacted)) = pending.take() {
                let body = if redacted {
                    ReasoningBody::Redacted
                } else {
                    ReasoningBody::Text {
                        text: BoundedString::truncating(&text),
                    }
                };
                let token = signature.map(|bytes| ReasoningToken {
                    provenance: provider,
                    bytes: bytes::Bytes::from(bytes),
                });
                blocks.push(CanonicalBlock::Reasoning(ReasoningBlock { body, token }));
            }
        };

    while let Some(item) = stream.next().await {
        if cancel.is_cancelled() {
            stream.cancel();
            return Err(StreamFailure::Cancelled);
        }
        match item {
            Ok(content) => {
                if !response_started.swap(true, Ordering::Relaxed) {
                    on_started().await?;
                }
                if first_item_at.is_none() {
                    first_item_at = Some(Instant::now());
                }
                match content {
                    StreamedAssistantContent::Text(delta) => {
                        let block_index = block_index(&blocks, text.as_ref(), reasoning.as_ref());
                        preview.offer(PreviewFrame::TextDelta {
                            index: u16::try_from(block_index).unwrap_or(0),
                            text: BoundedString::truncating(&delta.text),
                        });
                        text.get_or_insert_with(String::new).push_str(&delta.text);
                    }
                    StreamedAssistantContent::Reasoning(complete) => {
                        flush_text(&mut blocks, &mut text);
                        let (content, signature, redacted) = fold_reasoning_content(&complete);
                        let block_index = block_index(&blocks, text.as_ref(), reasoning.as_ref());
                        preview.offer(PreviewFrame::ReasoningDelta {
                            index: u16::try_from(block_index).unwrap_or(0),
                            text: BoundedString::truncating(&content),
                        });
                        reasoning = Some((content, signature, redacted));
                    }
                    StreamedAssistantContent::ReasoningDelta {
                        reasoning: delta, ..
                    } => {
                        let block_index = block_index(&blocks, text.as_ref(), reasoning.as_ref());
                        preview.offer(PreviewFrame::ReasoningDelta {
                            index: u16::try_from(block_index).unwrap_or(0),
                            text: BoundedString::truncating(&delta),
                        });
                        reasoning
                            .get_or_insert_with(|| (String::new(), None, false))
                            .0
                            .push_str(&delta);
                    }
                    StreamedAssistantContent::ToolCall {
                        tool_call,
                        internal_call_id,
                    } => {
                        flush_text(&mut blocks, &mut text);
                        flush_reasoning(&mut blocks, &mut reasoning);
                        preview.offer(PreviewFrame::ToolCallStart {
                            index: u16::try_from(blocks.len()).unwrap_or(0),
                            id: ToolCallId::truncating(&tool_call.id),
                            name: preview_tool_name(&tool_call.function.name),
                        });
                        let input = canonical_json(&tool_call.function.arguments)?;
                        blocks.push(CanonicalBlock::ToolUse {
                            id: ToolCallId::truncating(&tool_call.id),
                            name: preview_tool_name(&tool_call.function.name),
                            input,
                        });
                        preview.offer(PreviewFrame::BlockStop {
                            index: u16::try_from(blocks.len() - 1).unwrap_or(0),
                        });
                        let _ = internal_call_id;
                    }
                    StreamedAssistantContent::ToolCallDelta { id, content, .. } => match content {
                        ToolCallDeltaContent::Name(name) => {
                            preview.offer(PreviewFrame::ToolCallStart {
                                index: u16::try_from(blocks.len()).unwrap_or(0),
                                id: ToolCallId::truncating(&id),
                                name: preview_tool_name(&name),
                            });
                        }
                        ToolCallDeltaContent::Delta(delta) => {
                            preview.offer(PreviewFrame::ToolArgumentsDelta {
                                index: u16::try_from(blocks.len()).unwrap_or(0),
                                fragment: BoundedString::truncating(&delta),
                            });
                        }
                    },
                    StreamedAssistantContent::Final(response) => {
                        let usage = normalize_usage(&stream.usage());
                        preview.offer(PreviewFrame::InterimUsage(usage));
                        let _ = response;
                    }
                    StreamedAssistantContent::Unknown(_) => {
                        // A provider-native item this version does not model.
                        // Never becomes history; never fails the effect.
                    }
                }
            }
            Err(error) => {
                return Err(classify_stream_error(&error));
            }
        }
    }

    flush_text(&mut blocks, &mut text);
    flush_reasoning(&mut blocks, &mut reasoning);

    if blocks.is_empty() {
        return Err(StreamFailure::Protocol {
            detail: "the provider stream produced no canonical blocks".to_owned(),
        });
    }

    let usage = normalize_usage(&stream.usage());
    let stop = if blocks
        .iter()
        .any(|block| matches!(block, CanonicalBlock::ToolUse { .. }))
    {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    };

    Ok(StreamOutcome {
        blocks,
        usage,
        stop,
        first_item_at,
    })
}

fn block_index(
    blocks: &[CanonicalBlock],
    text: Option<&String>,
    reasoning: Option<&(String, Option<String>, bool)>,
) -> usize {
    blocks.len() + usize::from(text.is_some()) + usize::from(reasoning.is_some())
}

/// Folds a complete reasoning item into its text, signature and redaction.
fn fold_reasoning_content(
    complete: &rig_core::completion::message::Reasoning,
) -> (String, Option<String>, bool) {
    let mut text = String::new();
    let mut signature: Option<String> = None;
    let mut redacted = false;
    for part in &complete.content {
        match part {
            ReasoningContent::Text {
                text: part_text,
                signature: part_signature,
            } => {
                text.push_str(part_text);
                if part_signature.is_some() {
                    signature.clone_from(part_signature);
                }
            }
            ReasoningContent::Redacted { .. } => redacted = true,
            // A content form this version does not model: folded as text-free.
            _ => {}
        }
    }
    (text, signature, redacted)
}

/// A provider tool name that fails AEX's resource-name vocabulary still
/// reaches the preview with an honest placeholder rather than vanishing.
fn preview_tool_name(name: &str) -> ResourceName {
    ResourceName::parse(name)
        .ok()
        .unwrap_or_else(|| ResourceName::parse("unknown_tool").expect("the placeholder parses"))
}

/// Canonicalizes provider tool arguments. A provider that produces non-JSON
/// arguments is a protocol violation, never silently accepted.
fn canonical_json(value: &serde_json::Value) -> Result<CanonicalJson, StreamFailure> {
    let bytes = to_jcs_bytes(value).map_err(|_| StreamFailure::Protocol {
        detail: "provider tool arguments cannot be canonicalized".to_owned(),
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| StreamFailure::Protocol {
        detail: "provider tool arguments are not UTF-8".to_owned(),
    })?;
    CanonicalJson::parse(text).map_err(|error| StreamFailure::Protocol {
        detail: format!("provider tool arguments are not JSON: {error}"),
    })
}

/// rig usage → canonical normalized usage.
fn normalize_usage(usage: &Usage) -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: usage.input_tokens,
        cache_read_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_creation_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        tool_use_prompt_tokens: usage.tool_use_prompt_tokens,
        provider_total_tokens: Some(usage.total_tokens),
        completeness: UsageCompleteness::Exact,
    }
}

/// Classifies a rig completion error into the retry/terminal failure shape.
fn classify_stream_error(error: &rig_core::completion::CompletionError) -> StreamFailure {
    match error.provider_response_status() {
        Some(status) => StreamFailure::Definitive {
            status: status.as_u16(),
            detail: error
                .provider_response_body()
                .unwrap_or_default()
                .to_owned(),
        },
        None => StreamFailure::Transport {
            detail: error.to_string(),
        },
    }
}
