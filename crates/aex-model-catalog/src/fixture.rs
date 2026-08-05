//! Deterministic catalog construction, for tests, the publishing tool and the
//! live conformance harness.
//!
//! Following `aex_wire::testing`, this is an ordinary always-on module rather
//! than a feature-gated one: a test-only feature is exactly the sort of build
//! flag `00-orchestrator-conventions.md` forbids, and everything here builds a
//! *document*, which is data, not a bypass of any check. [`entry`] always
//! produces `Staged`; callers must promote it explicitly when testing signed
//! `Active` metadata.

use aex_wire::provider::ProviderId;
use aex_wire::to_jcs_bytes;
use aex_wire::types::Timestamp;

use crate::canonical::UsageFieldSet;
use crate::document::{
    AdapterSourceDigest, CacheMode, CachePolicy, CacheReadSemantics, CapabilitySet,
    CatalogDocument, CatalogSequence, Dialect, DialectRevision, EndpointPin, EntryState,
    ErrorClassMap, ModelEntry, ModelLimits, NamePattern, PreDispatchRetryPolicy, PublisherId,
    ReasoningEncoding, ReasoningMode, ReasoningPolicy, ReasoningReplay, SCHEMA_VERSION,
    SamplingSupport, StatusClassRule, StopReasonMap, StreamUsageDelivery, StructuredOutputPolicy,
    ToolArgumentEncoding, ToolEncoding, ToolPolicy, UsageMapping,
};
use crate::failure::ProviderFailureKind;
use crate::primitives::{Blake3Digest, BoundedString, ModelSlug};
use crate::qualified::QualifiedModel;
use crate::receipt::ProbeSuiteRevision;
use crate::wire_pending::CatalogRevision;

/// The probe-suite revision every fixture is stamped with.
pub const SUITE: ProbeSuiteRevision = ProbeSuiteRevision(1);

/// Mints a [`QualifiedModel`] directly from an entry, without a signed document.
///
/// The only production path to a `QualifiedModel` is [`crate::Catalog::load`],
/// which requires a valid ECDSA P-256 signature — and signing needs
/// `aws-lc-rs`, which the provider gateway does not and should not depend on.
/// Without this, no adapter test could reach `build_request` at all.
///
/// This is a **fixture, not a bypass**. It produces the handle and nothing
/// more. Every gate that decides admissibility — `EntryState` and emergency
/// disable — lives in [`crate::Catalog::admit`], which this function neither
/// touches nor can reach.
#[must_use]
pub fn qualified(entry: ModelEntry) -> QualifiedModel {
    let revision = CatalogRevision(Blake3Digest::of(
        format!("fixture/{}/{}", entry.provider, entry.model).as_bytes(),
    ));
    QualifiedModel::new(std::sync::Arc::new(entry), revision)
}

/// The [`QualifiedModel`] for a `Staged` fixture entry, in one call.
///
/// # Panics
///
/// As [`entry`].
#[must_use]
pub fn qualified_entry(
    provider: ProviderId,
    model: &str,
    capabilities: CapabilitySet,
) -> QualifiedModel {
    qualified(entry(provider, model, capabilities))
}

/// A bounded string from a literal that is known to fit.
///
/// # Panics
///
/// Panics when the literal exceeds `N` bytes, which is a bug in the caller's
/// own source rather than a runtime condition.
#[must_use]
pub fn bounded<const N: usize>(text: &str) -> BoundedString<N> {
    BoundedString::new(text).unwrap_or_else(|error| panic!("fixture literal is too long: {error}"))
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

/// An adapter digest for external qualification fixtures.
#[must_use]
pub fn adapter(seed: &str) -> AdapterSourceDigest {
    AdapterSourceDigest(Blake3Digest::of(seed.as_bytes()))
}

/// The dialect and endpoint each provider launches on.
#[must_use]
pub const fn dialect_for(provider: ProviderId) -> (Dialect, EndpointPin) {
    match provider {
        ProviderId::Openai => (Dialect::OpenAiResponses, EndpointPin::OpenAiApi),
        ProviderId::Anthropic => (Dialect::AnthropicMessages, EndpointPin::AnthropicApi),
        ProviderId::Deepseek => (Dialect::DeepSeekChat, EndpointPin::DeepSeekApi),
        ProviderId::Zai => (Dialect::ZaiChat, EndpointPin::ZaiPaasV4),
        ProviderId::Moonshotai => (Dialect::MoonshotChat, EndpointPin::MoonshotIntlV1),
        ProviderId::Google => (Dialect::GeminiGenerateContent, EndpointPin::GeminiV1Beta),
        ProviderId::Openrouter => (Dialect::OpenRouterChat, EndpointPin::OpenRouterApiV1),
        ProviderId::VercelAiGateway => {
            (Dialect::VercelAiGatewayChat, EndpointPin::VercelAiGatewayV1)
        }
    }
}

/// A minimal, self-consistent `Staged` entry.
///
/// A staged entry is described but inadmissible until a signed publisher marks
/// it `Active`.
///
/// # Panics
///
/// Panics when `model` exceeds the model-slug bound, which is a bug in the
/// caller's own source rather than a runtime condition.
#[must_use]
pub fn entry(provider: ProviderId, model: &str, capabilities: CapabilitySet) -> ModelEntry {
    let (dialect, endpoint) = dialect_for(provider);
    let model = ModelSlug::new(model).unwrap_or_else(|error| panic!("model slug: {error}"));
    ModelEntry {
        provider,
        model,
        state: EntryState::Staged,
        dialect,
        dialect_revision: DialectRevision(1),
        endpoint,
        capabilities,
        limits: limits(),
        sampling: SamplingSupport::Full,
        reasoning: ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::None,
            replay: ReasoningReplay::NotRequired,
            excludes_sampling: false,
        },
        structured_output: StructuredOutputPolicy::JsonObjectOnly,
        tool_policy: ToolPolicy {
            encoding: ToolEncoding::OpenAiNestedFunction,
            arguments: ToolArgumentEncoding::JsonString,
            requires_stream_opt_in: false,
            max_name_bytes: 64,
            name_pattern: NamePattern::OpenAiFunctionName,
        },
        cache_policy: CachePolicy {
            mode: CacheMode::Implicit,
            max_breakpoints: 0,
        },
        usage_map: UsageMapping {
            reasoning_included_in_output: provider != ProviderId::Google,
            cache_read_field: CacheReadSemantics::CachedTokensSubsetOfInput,
            stream_usage_delivery: StreamUsageDelivery::TerminalEvent,
            known_missing: UsageFieldSet::EMPTY,
        },
        stop_reason_map: stop_reason_map(),
        error_map: error_class_map(),
        retry_policy: PreDispatchRetryPolicy::conservative(),
        durable_operation: crate::document::DurableOperationSupport::None,
        concurrency_hint: 8,
        pricing_context: None,
    }
}

/// Bounds wide enough that a fixture never trips one by accident.
#[must_use]
pub fn limits() -> ModelLimits {
    ModelLimits {
        context_window_tokens: 200_000,
        max_output_tokens: 8_192,
        min_output_tokens: 1,
        max_reasoning_tokens: Some(4_096),
        min_reasoning_tokens: Some(1_024),
        max_tools: 128,
        max_stop_sequences: 4,
        temperature_milli: Some((0, 1_000)),
        top_p_milli: Some((10, 1_000)),
        min_cacheable_prefix_tokens: 1_024,
        request_body_max_bytes: 32 * 1024 * 1024,
        response_frame_max_bytes: 1024 * 1024,
        stream_idle_timeout_ms: 60_000,
        total_stream_deadline_ms: 900_000,
    }
}

/// The `OpenAI`-family finish-token map, which four of the six dialects share.
#[must_use]
pub fn stop_reason_map() -> StopReasonMap {
    StopReasonMap {
        end_turn: vec![bounded("stop")],
        tool_use: vec![bounded("tool_calls")],
        max_output_tokens: vec![bounded("length")],
        stop_sequence: Vec::new(),
        refusal: Vec::new(),
        failure: Vec::new(),
    }
}

/// A conservative status map.
#[must_use]
pub fn error_class_map() -> ErrorClassMap {
    ErrorClassMap {
        status: vec![
            StatusClassRule {
                status: 401,
                kind: ProviderFailureKind::Authentication,
            },
            StatusClassRule {
                status: 429,
                kind: ProviderFailureKind::RateLimited,
            },
            StatusClassRule {
                status: 503,
                kind: ProviderFailureKind::Overloaded,
            },
        ],
        codes: Vec::new(),
        default_kind: ProviderFailureKind::ServerError,
    }
}

/// Marks an entry `Active` for signed catalog fixtures.
pub fn promote(entry: &mut ModelEntry) {
    entry.state = EntryState::Active;
}

/// A document over the supplied entries, with the entry and disable lists
/// sorted into the order the loader requires.
#[must_use]
pub fn document(
    publisher: &str,
    sequence: u64,
    mut entries: Vec<ModelEntry>,
    now: Timestamp,
) -> CatalogDocument {
    entries.sort_by(|left, right| {
        (left.provider, left.model.as_str()).cmp(&(right.provider, right.model.as_str()))
    });
    CatalogDocument {
        schema_version: SCHEMA_VERSION,
        publisher: PublisherId(bounded(publisher)),
        sequence: CatalogSequence(sequence),
        predecessor: None,
        issued_at: now,
        not_before: now,
        entries,
        emergency_disable: Vec::new(),
    }
}

/// The canonical (JCS) bytes of a document. This is what gets signed and what
/// the digest is taken over.
///
/// # Panics
///
/// Panics when the document cannot be canonicalized, which no well-formed
/// document can trigger.
#[must_use]
pub fn canonical_bytes(document: &CatalogDocument) -> bytes::Bytes {
    let bytes = to_jcs_bytes(document)
        .unwrap_or_else(|error| panic!("a well-formed document canonicalizes: {error}"));
    bytes::Bytes::from(bytes)
}
