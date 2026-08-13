//! The canonical vocabulary, extended with the one type that must stay in
//! this crate: [`CanonicalModelRequest`], whose `selection` is a live
//! [`QualifiedModel`] handle into the admitted model table.
//!
//! Everything provider-neutral lives in `aex-model-vocabulary` and is
//! re-exported here so existing `aex_model_catalog::canonical::*` paths
//! resolve unchanged.

pub use aex_model_vocabulary::canonical::*;

use aex_wire::provider::ProviderId;
use aex_wire::{ContentHash, to_jcs_bytes};
use serde::Serialize;

use crate::primitives::{BoundedString, ModelSlug};
use crate::qualified::QualifiedModel;
use crate::wire_pending::CatalogRevision;

/// The identity and policy facts `seal` needs, from a [`QualifiedModel`].
impl SealModel for QualifiedModel {
    fn provider(&self) -> ProviderId {
        self.provider()
    }

    fn model(&self) -> &ModelSlug {
        self.model()
    }

    fn catalog(&self) -> CatalogRevision {
        self.catalog()
    }

    fn requires_reasoning_token(&self, has_tool_use: bool) -> bool {
        self.requires_reasoning_token(has_tool_use)
    }
}

/// A provider-neutral generation request.
///
/// Deliberately **not** `Serialize`/`Deserialize`: it carries a
/// [`QualifiedModel`], which is a live handle into the admitted model table
/// rather than a wire value. What is hashed and journalled is
/// [`CanonicalModelRequest::digest`]'s projection, not the struct itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalModelRequest {
    /// The qualified `(provider, model)` pair. Already admitted.
    pub selection: QualifiedModel,
    /// System instruction blocks.
    pub system: Vec<SystemBlock>,
    /// Conversation history.
    pub messages: Vec<CanonicalMessage>,
    /// Tool declarations.
    pub tools: Vec<CanonicalToolDef>,
    /// How the model should choose among the tools.
    pub tool_choice: ToolChoice,
    /// Whether parallel tool calls are permitted.
    pub parallel_tools: bool,
    /// The output ceiling for this call.
    pub max_output_tokens: u32,
    /// Temperature in integer milli-units, never a float.
    pub temperature_milli: Option<u16>,
    /// Nucleus sampling in integer milli-units.
    pub top_p_milli: Option<u16>,
    /// Caller stop sequences.
    pub stop_sequences: Vec<BoundedString<64>>,
    /// Reasoning request.
    pub reasoning: ReasoningRequest,
    /// Structured-output request.
    pub structured_output: Option<StructuredOutputRequest>,
    /// Explicit prompt-cache breakpoints.
    pub cache_breakpoints: Vec<CacheBreakpoint>,
    /// A non-secret correlation handle derived from the effect id.
    pub correlation: CorrelationId,
    /// The content hash of the canonical form of this request.
    pub request_hash: ContentHash,
}

/// The serializable projection of a request, and the only thing ever hashed or
/// exported. `selection` collapses to the three facts that identify the pair;
/// the live catalog handle does not travel.
#[derive(Debug, Serialize)]
struct RequestDigestInput<'a> {
    cache_breakpoints: &'a [CacheBreakpoint],
    catalog: CatalogRevision,
    correlation: &'a CorrelationId,
    max_output_tokens: u32,
    messages: &'a [CanonicalMessage],
    model: &'a ModelSlug,
    parallel_tools: bool,
    provider: ProviderId,
    reasoning: &'a ReasoningRequest,
    stop_sequences: &'a [BoundedString<64>],
    structured_output: &'a Option<StructuredOutputRequest>,
    system: &'a [SystemBlock],
    temperature_milli: Option<u16>,
    tool_choice: &'a ToolChoice,
    tools: &'a [CanonicalToolDef],
    top_p_milli: Option<u16>,
}

impl CanonicalModelRequest {
    /// The content hash over the canonical projection of this request.
    ///
    /// Every field except `request_hash` itself participates, so two requests
    /// that would produce different provider bodies cannot share a hash.
    ///
    /// # Errors
    ///
    /// Returns [`SealError::InvalidToolInputJson`] when a member cannot be
    /// canonicalized. Every member is an already-validated bounded type, so
    /// this is a total-function guard rather than a reachable path.
    pub fn digest(&self) -> Result<ContentHash, SealError> {
        let input = RequestDigestInput {
            cache_breakpoints: &self.cache_breakpoints,
            catalog: self.selection.catalog(),
            correlation: &self.correlation,
            max_output_tokens: self.max_output_tokens,
            messages: &self.messages,
            model: self.selection.model(),
            parallel_tools: self.parallel_tools,
            provider: self.selection.provider(),
            reasoning: &self.reasoning,
            stop_sequences: &self.stop_sequences,
            structured_output: &self.structured_output,
            system: &self.system,
            temperature_milli: self.temperature_milli,
            tool_choice: &self.tool_choice,
            tools: &self.tools,
            top_p_milli: self.top_p_milli,
        };
        let bytes = to_jcs_bytes(&input).map_err(|_| SealError::InvalidToolInputJson)?;
        Ok(ContentHash::of(&bytes))
    }

    /// Whether `request_hash` matches the request it claims to describe.
    ///
    /// # Errors
    ///
    /// As [`CanonicalModelRequest::digest`].
    pub fn hash_is_consistent(&self) -> Result<bool, SealError> {
        Ok(self.digest()? == self.request_hash)
    }
}
