//! Deterministic catalog construction for tests.
//!
//! Following `aex_wire::testing`, this is an ordinary always-on module rather
//! than a feature-gated one: a test-only feature is exactly the sort of build
//! flag `00-orchestrator-conventions.md` forbids, and everything here builds a
//! *fixture row*, which is data, not a bypass of any check.

use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;

use crate::document::{CapabilitySet, ModelLimits};
use crate::generated::{AdmittedModel, MODELS, SNAPSHOT_DIGEST};
use crate::primitives::ModelSlug;
use crate::qualified::QualifiedModel;
use crate::wire_pending::CatalogRevision;
use aex_model_vocabulary::{Blake3Digest, DialectClass};

/// The revision every fixture handle is stamped with.
pub const FIXTURE_REVISION: CatalogRevision = CatalogRevision(Blake3Digest::from_bytes([7; 32]));

/// Mints a [`QualifiedModel`] directly from an admitted row.
///
/// The only production path to a `QualifiedModel` is [`crate::qualified::admit`],
/// which consults the compiled table; fixtures mint handles over rows they
/// leak themselves so arbitrary pairs are testable without touching the table.
///
/// # Panics
///
/// Panics when `model` exceeds the model-slug bound, which is a bug in the
/// caller's own source rather than a runtime condition.
#[must_use]
pub fn qualified(entry: &'static AdmittedModel, model: &str) -> QualifiedModel {
    let slug = ModelSlug::new(model)
        .unwrap_or_else(|error| panic!("fixture model slug is invalid: {error}"));
    QualifiedModel::new(entry, slug, FIXTURE_REVISION)
}

/// The [`QualifiedModel`] for a fixture row with the given capabilities, in
/// one call.
#[must_use]
pub fn qualified_entry(
    provider: ProviderId,
    model: &str,
    capabilities: CapabilitySet,
) -> QualifiedModel {
    qualified_entry_sized(provider, model, capabilities, 200_000, 8_192)
}

/// The [`QualifiedModel`] for a fixture row with explicit limits, so a test
/// can craft an exact window without touching the compiled table.
#[must_use]
pub fn qualified_entry_sized(
    provider: ProviderId,
    model: &str,
    capabilities: CapabilitySet,
    context_window_tokens: u32,
    max_output_tokens: u32,
) -> QualifiedModel {
    qualified_entry_sized_and_output(
        provider,
        model,
        capabilities,
        context_window_tokens,
        max_output_tokens,
        if capabilities.has(crate::document::Capability::StructuredOutput) {
            crate::document::StructuredOutputLevel::JsonSchema
        } else {
            crate::document::StructuredOutputLevel::None
        },
    )
}

/// The [`QualifiedModel`] for a fixture row with an exact structured-output
/// level, for transport qualification tests.
#[must_use]
pub fn qualified_entry_sized_and_output(
    provider: ProviderId,
    model: &str,
    capabilities: CapabilitySet,
    context_window_tokens: u32,
    max_output_tokens: u32,
    structured_output: crate::document::StructuredOutputLevel,
) -> QualifiedModel {
    qualified(
        Box::leak(Box::new(AdmittedModel {
            provider,
            model: Box::leak(model.to_owned().into_boxed_str()),
            context_window_tokens,
            max_output_tokens,
            tools: capabilities.has(crate::document::Capability::Tools),
            parallel_tools: capabilities.has(crate::document::Capability::ParallelTools),
            structured_output,
            dialect: fixture_dialect(provider),
        })),
        model,
    )
}

/// The dialect class each provider launches on, for fixtures.
#[must_use]
pub const fn fixture_dialect(provider: ProviderId) -> DialectClass {
    match provider {
        ProviderId::Openai => DialectClass::OpenAiResponses,
        ProviderId::Anthropic => DialectClass::AnthropicMessages,
        ProviderId::Deepseek => DialectClass::DeepSeekChat,
        ProviderId::Xai => DialectClass::XAiResponses,
        ProviderId::Meta => DialectClass::MetaChat,
        ProviderId::Moonshotai => DialectClass::MoonshotChat,
        ProviderId::Alibaba => DialectClass::AlibabaChat,
    }
}

/// A bounded string from a literal that is known to fit.
///
/// # Panics
///
/// Panics when the literal exceeds `N` bytes, which is a bug in the caller's
/// own source rather than a runtime condition.
#[must_use]
pub fn bounded<const N: usize>(text: &str) -> aex_model_vocabulary::BoundedString<N> {
    aex_model_vocabulary::BoundedString::new(text)
        .unwrap_or_else(|error| panic!("fixture literal is too long: {error}"))
}

/// A timestamp from Unix milliseconds.
///
/// # Panics
///
/// Panics when the value is outside the wire range.
#[must_use]
pub fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|error| panic!("fixture timestamp is out of range: {error}"))
}

/// The compiled table itself: fixtures may resolve real rows against it.
#[must_use]
pub const fn table() -> &'static [AdmittedModel] {
    MODELS
}

/// The compiled snapshot digest bytes.
#[must_use]
pub const fn snapshot_digest() -> [u8; 32] {
    SNAPSHOT_DIGEST
}

/// Fixture limits matching the compiled rows' shape.
#[must_use]
pub const fn limits() -> ModelLimits {
    ModelLimits {
        context_window_tokens: 200_000,
        max_output_tokens: 8_192,
    }
}
