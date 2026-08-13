//! Canonical request → rig completion request.
//!
//! The canonical request is already provider-neutral and validated; this
//! module is the one translation site for prompt shape, tools, tool choice,
//! sampling and stop sequences.

use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef, Role,
    StructuredOutputRequest, ToolChoice as CanonicalToolChoice, ToolResultPart,
};
use aex_model_catalog::document::{Capability, StructuredOutputLevel};
use aex_model_vocabulary::DialectClass;
use rig_core::OneOrMany;
use rig_core::completion::message::{
    AssistantContent, Message, Reasoning, Text, ToolCall, ToolChoice, ToolFunction,
    ToolResultContent, UserContent,
};
use rig_core::completion::{CompletionRequest, ToolDefinition};

/// A rig request is never built from an inconsistent canonical request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestBuildError {
    /// A canonical block could not be translated into provider content.
    #[error("canonical request member cannot be translated: {0}")]
    Untranslatable(&'static str),
    /// The current model/Rig route did not certify a requested capability.
    #[error("the qualified model does not support {0}")]
    UnsupportedCapability(&'static str),
    /// A canonical JSON value was not a JSON Schema object or boolean.
    #[error("the structured-output schema is invalid")]
    InvalidSchema,
}

/// Builds the rig completion request for one dispatch.
///
/// # Errors
///
/// Returns [`RequestBuildError`] when a member of the canonical request cannot
/// be carried verbatim (for example, a tool schema that is not JSON, which is
/// already excluded by the canonical type).
pub fn build(
    request: &CanonicalModelRequest,
    dialect: DialectClass,
) -> Result<CompletionRequest, RequestBuildError> {
    validate_capabilities(request)?;
    let mut history: Vec<Message> = Vec::new();
    for block in &request.system {
        history.push(Message::System {
            content: block.text.as_str().to_owned(),
        });
    }
    for message in &request.messages {
        history.push(translate_message(message)?);
    }

    let (output_schema, json_object_only) = structured_output(request)?;
    Ok(CompletionRequest {
        model: Some(request.selection.model().as_str().to_owned()),
        preamble: None,
        chat_history: if history.is_empty() {
            OneOrMany::one(Message::User {
                content: OneOrMany::one(UserContent::text("")),
            })
        } else {
            OneOrMany::many(history)
                .map_err(|_| RequestBuildError::Untranslatable("too many history blocks"))?
        },
        documents: Vec::new(),
        tools: request
            .tools
            .iter()
            .map(translate_tool)
            .collect::<Result<Vec<_>, _>>()?,
        temperature: request
            .temperature_milli
            .map(|milli| f64::from(milli) / 1000.0),
        max_tokens: Some(u64::from(request.max_output_tokens)),
        tool_choice: Some(translate_tool_choice(&request.tool_choice)),
        additional_params: additional_params(request, dialect, json_object_only),
        output_schema,
        record_telemetry_content: false,
    })
}

fn validate_capabilities(request: &CanonicalModelRequest) -> Result<(), RequestBuildError> {
    let capabilities = request.selection.capabilities();
    if !request.tools.is_empty() && !capabilities.has(Capability::Tools) {
        return Err(RequestBuildError::UnsupportedCapability("tool calling"));
    }
    if request.parallel_tools && !capabilities.has(Capability::ParallelTools) {
        return Err(RequestBuildError::UnsupportedCapability(
            "parallel tool calling",
        ));
    }
    match (&request.structured_output, request.selection.structured_output()) {
        (None, _) => Ok(()),
        (Some(StructuredOutputRequest::JsonObject), StructuredOutputLevel::None) => Err(
            RequestBuildError::UnsupportedCapability("structured JSON output"),
        ),
        (Some(StructuredOutputRequest::JsonSchema { .. }), StructuredOutputLevel::JsonSchema) => {
            Ok(())
        }
        (Some(StructuredOutputRequest::JsonSchema { .. }), _) => Err(
            RequestBuildError::UnsupportedCapability("native JSON Schema output"),
        ),
        (Some(StructuredOutputRequest::JsonObject), _) => Ok(()),
    }
}

fn structured_output(
    request: &CanonicalModelRequest,
) -> Result<(Option<schemars::Schema>, bool), RequestBuildError> {
    let Some(output) = &request.structured_output else {
        return Ok((None, false));
    };
    match output {
        StructuredOutputRequest::JsonObject => {
            if request.selection.structured_output() == StructuredOutputLevel::JsonObject {
                return Ok((None, true));
            }
            let value = serde_json::json!({
                "title": "response",
                "type": "object"
            });
            Ok((Some(value.try_into().map_err(|_| RequestBuildError::InvalidSchema)?), false))
        }
        StructuredOutputRequest::JsonSchema { name, schema, .. } => {
            let mut value: serde_json::Value = serde_json::from_str(schema.as_str())
                .map_err(|_| RequestBuildError::InvalidSchema)?;
            let object = value
                .as_object_mut()
                .ok_or(RequestBuildError::InvalidSchema)?;
            object
                .entry("title".to_owned())
                .or_insert_with(|| serde_json::Value::String(name.to_string()));
            Ok((Some(value.try_into().map_err(|_| RequestBuildError::InvalidSchema)?), false))
        }
    }
}

fn translate_message(message: &CanonicalMessage) -> Result<Message, RequestBuildError> {
    match message.role {
        Role::User => {
            let mut content: Vec<UserContent> = Vec::new();
            for block in &message.blocks {
                match block {
                    CanonicalBlock::Text { text, .. } => {
                        content.push(UserContent::text(text.as_str()));
                    }
                    CanonicalBlock::Refusal { text } => {
                        content.push(UserContent::text(text.as_str()));
                    }
                    CanonicalBlock::ToolResult {
                        call,
                        content: result_parts,
                        ..
                    } => {
                        let parts = translate_tool_result_parts(result_parts)?;
                        content.push(UserContent::tool_result(call.as_str(), parts));
                    }
                    CanonicalBlock::Reasoning(_) | CanonicalBlock::ToolUse { .. } => {
                        return Err(RequestBuildError::Untranslatable(
                            "assistant-only block in a user message",
                        ));
                    }
                }
            }
            let content = if content.is_empty() {
                OneOrMany::one(UserContent::text(""))
            } else {
                OneOrMany::many(content)
                    .map_err(|_| RequestBuildError::Untranslatable("too many user blocks"))?
            };
            Ok(Message::User { content })
        }
        Role::Assistant => {
            let mut content: Vec<AssistantContent> = Vec::new();
            for block in &message.blocks {
                match block {
                    CanonicalBlock::Text { text, .. } => {
                        content.push(AssistantContent::text(text.as_str()));
                    }
                    CanonicalBlock::Refusal { text } => {
                        content.push(AssistantContent::text(text.as_str()));
                    }
                    CanonicalBlock::Reasoning(reasoning) => {
                        let signature = reasoning.token.as_ref().and_then(|token| {
                            std::str::from_utf8(&token.bytes)
                                .ok()
                                .map(std::borrow::ToOwned::to_owned)
                        });
                        let assembled = match &reasoning.body {
                            aex_model_catalog::canonical::ReasoningBody::Text { text } => {
                                let mut part =
                                    Reasoning::new_with_signature(text.as_str(), signature.clone());
                                if signature.is_none() {
                                    part = Reasoning::new(text.as_str());
                                }
                                part
                            }
                            aex_model_catalog::canonical::ReasoningBody::Summary { text } => {
                                Reasoning::new(text.as_str())
                            }
                            aex_model_catalog::canonical::ReasoningBody::Redacted => {
                                Reasoning::redacted("")
                            }
                        };
                        content.push(AssistantContent::Reasoning(assembled));
                    }
                    CanonicalBlock::ToolUse { id, name, input } => {
                        let arguments: serde_json::Value = serde_json::from_str(input.as_str())
                            .map_err(|_| {
                                RequestBuildError::Untranslatable("tool input is not JSON")
                            })?;
                        content.push(AssistantContent::ToolCall(ToolCall::new(
                            id.as_str().to_owned(),
                            ToolFunction::new(name.as_str().to_owned(), arguments),
                        )));
                    }
                    CanonicalBlock::ToolResult { .. } => {
                        return Err(RequestBuildError::Untranslatable(
                            "tool result in an assistant message",
                        ));
                    }
                }
            }
            let content = if content.is_empty() {
                OneOrMany::one(AssistantContent::text(""))
            } else {
                OneOrMany::many(content)
                    .map_err(|_| RequestBuildError::Untranslatable("too many assistant blocks"))?
            };
            Ok(Message::Assistant { id: None, content })
        }
    }
}

fn translate_tool(tool: &CanonicalToolDef) -> Result<ToolDefinition, RequestBuildError> {
    let parameters: serde_json::Value = serde_json::from_str(tool.input_schema.as_str())
        .map_err(|_| RequestBuildError::Untranslatable("tool schema is not JSON"))?;
    Ok(ToolDefinition {
        name: tool.name.as_str().to_owned(),
        description: tool.description.as_str().to_owned(),
        parameters,
    })
}

fn translate_tool_choice(choice: &CanonicalToolChoice) -> ToolChoice {
    match choice {
        CanonicalToolChoice::Auto => ToolChoice::Auto,
        CanonicalToolChoice::None => ToolChoice::None,
        CanonicalToolChoice::Required => ToolChoice::Required,
        CanonicalToolChoice::Named { name } => ToolChoice::Specific {
            function_names: vec![name.as_str().to_owned()],
        },
    }
}

/// Canonical tool-result parts → rig tool-result content.
fn translate_tool_result_parts(
    parts: &[ToolResultPart],
) -> Result<OneOrMany<ToolResultContent>, RequestBuildError> {
    let mut content: Vec<ToolResultContent> = Vec::new();
    for part in parts {
        match part {
            ToolResultPart::Text { text } => {
                content.push(ToolResultContent::Text(Text::new(text.as_str())));
            }
            ToolResultPart::Json { value } => {
                let parsed: serde_json::Value = serde_json::from_str(value.as_str())
                    .map_err(|_| RequestBuildError::Untranslatable("tool result is not JSON"))?;
                content.push(ToolResultContent::Json { value: parsed });
            }
        }
    }
    if content.is_empty() {
        Ok(OneOrMany::one(ToolResultContent::Text(Text::new(""))))
    } else {
        OneOrMany::many(content)
            .map_err(|_| RequestBuildError::Untranslatable("too many tool-result parts"))
    }
}

/// The provider-specific sampling and stop parameters.
///
/// rig models temperature and `max_tokens` natively; `top_p` and stop
/// sequences travel through `additional_params`, which the OpenAI-compatible
/// serialization flattens into the request body. Anthropic and Gemini keep
/// rig's own defaults for these two parameters: their serializers do not
/// flatten arbitrary `additional_params`, so nothing is invented.
fn additional_params(
    request: &CanonicalModelRequest,
    dialect: DialectClass,
    json_object_only: bool,
) -> Option<serde_json::Value> {
    let mut params = serde_json::Map::new();
    if let Some(top_p_milli) = request.top_p_milli {
        params.insert(
            "top_p".to_owned(),
            serde_json::json!(f64::from(top_p_milli) / 1000.0),
        );
    }
    if !request.stop_sequences.is_empty()
        && matches!(
            dialect,
            DialectClass::OpenAiResponses
                | DialectClass::DeepSeekChat
                | DialectClass::XAiResponses
                | DialectClass::MetaChat
                | DialectClass::MoonshotChat
                | DialectClass::AlibabaChat
        )
    {
        params.insert(
            "stop".to_owned(),
            serde_json::json!(
                request
                    .stop_sequences
                    .iter()
                    .map(aex_model_catalog::BoundedString::as_str)
                    .collect::<Vec<_>>()
            ),
        );
    }
    if json_object_only {
        params.insert(
            "response_format".to_owned(),
            serde_json::json!({ "type": "json_object" }),
        );
    }
    if params.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(params))
    }
}
