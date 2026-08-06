//! Canonical external assurance from a `DeepSeek` qualification matrix.
//!
//! Qualification evidence is deliberately separate from catalog authority.
//! This module can emit a standalone [`ConformanceReceipt`], but it cannot
//! manufacture or mutate a signed compatibility catalog document.

use std::collections::BTreeSet;

use aex_brain_provider_gateway::build_identity::{
    AdapterBuildIdentityError, adapter_source_digest,
};
use aex_model_catalog::canonical::{NormalizedUsage, UsageFieldSet};
use aex_model_catalog::catalog::{CatalogLoadError, catalog_entry_digest};
use aex_model_catalog::document::{
    CacheMode, CachePolicy, CacheReadSemantics, Capability, CapabilitySet, Dialect,
    DialectRevision, DurableOperationSupport, EndpointPin, EntryState, ErrorClassMap, ModelEntry,
    ModelLimits, NamePattern, PreDispatchRetryPolicy, ReasoningEncoding, ReasoningMode,
    ReasoningPolicy, ReasoningReplay, SamplingSupport, StatusClassRule, StopFailureMapping,
    StopReasonMap, StreamUsageDelivery, StructuredOutputPolicy, ToolArgumentEncoding, ToolEncoding,
    ToolPolicy, UsageMapping,
};
use aex_model_catalog::failure::ProviderFailureKind;
use aex_model_catalog::primitives::{BoundError, BoundedString};
use aex_model_catalog::receipt::{
    ConformanceReceipt, PlaneId, ProbeId, ProbeSuiteRevision, RECEIPT_FRESHNESS_MS, Region,
};
use aex_wire::canonical::CanonicalError;
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, Uuid7, to_jcs_bytes};

use crate::{ProbeRun, ReceiptBuilder, ReceiptError, earns_active};

/// Exact provider-native model qualified by the genesis lane.
pub const DEEPSEEK_FLASH_MODEL: &str = "deepseek-v4-flash";

/// The deliberately small capability claim the genesis probe matrix earns.
pub const DEEPSEEK_GENESIS_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
]);

/// The only probe-suite revision this builder can publish.
pub const GENESIS_PROBE_SUITE_REVISION: ProbeSuiteRevision = ProbeSuiteRevision(1);

/// Explicit identities and timestamps supplied by the protected qualification run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisMetadata {
    /// Time-ordered identity for the receipt earned by this run.
    pub receipt_id: Uuid7,
    /// Plane in which the live matrix ran.
    pub plane: PlaneId,
    /// Region in which the live matrix ran.
    pub region: Region,
    /// Start time of the completed matrix.
    pub ran_at: Timestamp,
    /// Provider request ids retained for provider-side correlation.
    pub provider_request_ids: Vec<BoundedString<80>>,
    /// Exact aggregate usage observed by the live matrix.
    pub tokens_spent: NormalizedUsage,
    /// Content hash of the external, immutable evidence bundle.
    pub evidence_digest: ContentHash,
}

/// Standalone assurance output from the protected qualification lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisOutput {
    /// Receipt binding the exact reviewed Active compatibility projection.
    pub receipt: ConformanceReceipt,
    /// Canonical JCS encoding of [`Self::receipt`].
    pub canonical_receipt: Vec<u8>,
}

/// Why earned genesis output could not be constructed.
#[derive(Debug, thiserror::Error)]
pub enum GenesisError {
    /// The binary lacks the compile-time adapter source identity.
    #[error(transparent)]
    AdapterIdentity(#[from] AdapterBuildIdentityError),
    /// A reviewed static policy value no longer fits its schema bound.
    #[error("the reviewed static value for {field} is outside its schema bound: {source}")]
    StaticPolicy {
        /// Policy field whose reviewed value was rejected.
        field: &'static str,
        /// Closed bounded-string failure.
        #[source]
        source: BoundError,
    },
    /// Two runs claim to be the result for the same probe.
    #[error("the matrix contains more than one run for {probe:?}")]
    DuplicateProbe {
        /// Duplicated probe identity.
        probe: ProbeId,
    },
    /// The receipt builder rejected an incomplete or unrepresentable run.
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    /// At least one probe required by the claimed capabilities did not pass.
    #[error("the matrix did not earn Active; required probes failed: {probes:?}")]
    ActiveNotEarned {
        /// Exact failed or non-applicable required probes.
        probes: Vec<ProbeId>,
    },
    /// The supplied usage violates the canonical reasoning/output invariant.
    #[error("qualification usage is inconsistent: reasoning tokens exceed output tokens")]
    InconsistentUsage,
    /// A freshness or retirement gate is outside the wire timestamp range.
    #[error("a genesis time gate is outside the representable timestamp range")]
    TimeGateOutOfRange,
    /// Canonical JSON generation failed.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// The reviewed compatibility projection was not valid catalog metadata.
    #[error(transparent)]
    CatalogProjection(#[from] CatalogLoadError),
}

/// Constructs the single reviewed staged candidate used by qualification and publication.
///
/// The candidate is Staged because this helper is used by the live runner.
/// Static signed catalog source is reviewed and generated independently.
///
/// # Errors
///
/// Returns [`GenesisError::StaticPolicy`] if a reviewed literal stops fitting
/// the closed schema that carries it.
pub fn deepseek_v4_flash_candidate() -> Result<ModelEntry, GenesisError> {
    Ok(ModelEntry {
        provider: ProviderId::Deepseek,
        model: bounded("model", DEEPSEEK_FLASH_MODEL)?,
        state: EntryState::Staged,
        dialect: Dialect::DeepSeekChat,
        dialect_revision: DialectRevision(1),
        endpoint: EndpointPin::DeepSeekApi,
        capabilities: DEEPSEEK_GENESIS_CAPABILITIES,
        limits: ModelLimits {
            context_window_tokens: 1_000_000,
            max_output_tokens: 384_000,
            min_output_tokens: 1,
            max_reasoning_tokens: None,
            min_reasoning_tokens: None,
            max_tools: 128,
            max_stop_sequences: 16,
            temperature_milli: Some((0, 2_000)),
            top_p_milli: Some((0, 1_000)),
            min_cacheable_prefix_tokens: 64,
            request_body_max_bytes: 32 * 1024 * 1024,
            response_frame_max_bytes: 1024 * 1024,
            stream_idle_timeout_ms: 60_000,
            total_stream_deadline_ms: 900_000,
        },
        sampling: SamplingSupport::Full,
        reasoning: ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::DeepSeekThinking,
            replay: ReasoningReplay::RequiredWithToolCalls,
            excludes_sampling: true,
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
            reasoning_included_in_output: true,
            cache_read_field: CacheReadSemantics::HitMissSplit,
            stream_usage_delivery: StreamUsageDelivery::TrailingChoicesEmptyChunk,
            known_missing: UsageFieldSet::EMPTY,
        },
        stop_reason_map: reviewed_stop_reason_map()?,
        error_map: reviewed_error_map(),
        retry_policy: PreDispatchRetryPolicy::conservative(),
        durable_operation: DurableOperationSupport::None,
        concurrency_hint: 2_500,
        pricing_context: None,
    })
}

/// Turns one complete live matrix into standalone qualification assurance.
///
/// The current adapter identity is taken from the binary's build stamp. The
/// receipt binds the exact reviewed Active entry projection, while publication
/// remains solely the responsibility of the static source and protected signer.
///
/// # Errors
///
/// Rejects duplicate or missing probes, failed required probes, invalid usage
/// or usage, adapter identity failures, and unrepresentable receipt expiry.
pub fn build_deepseek_genesis(
    runs: Vec<ProbeRun>,
    metadata: GenesisMetadata,
) -> Result<GenesisOutput, GenesisError> {
    let GenesisMetadata {
        receipt_id,
        plane,
        region,
        ran_at,
        provider_request_ids,
        tokens_spent,
        evidence_digest,
    } = metadata;

    if !tokens_spent.is_consistent() {
        return Err(GenesisError::InconsistentUsage);
    }
    ensure_receipt_expiry_representable(ran_at)?;

    let mut seen = BTreeSet::new();
    for run in &runs {
        if !seen.insert(run.probe) {
            return Err(GenesisError::DuplicateProbe { probe: run.probe });
        }
    }

    let mut entry = deepseek_v4_flash_candidate()?;
    let adapter = adapter_source_digest()?;
    // `state` participates in the entry projection. Qualification binds the
    // reviewed Active compatibility entry without attaching assurance to it.
    entry.state = EntryState::Active;
    let entry_digest = catalog_entry_digest(&entry)?;
    let mut builder =
        ReceiptBuilder::new(adapter, GENESIS_PROBE_SUITE_REVISION, plane, region, ran_at);
    for run in runs {
        builder.record(run);
    }
    for request_id in provider_request_ids {
        builder.observe_request_id(request_id.as_str());
    }
    builder.spend(tokens_spent);
    let mut receipt = builder.build(entry_digest, evidence_digest, receipt_id)?;
    // This call receives one already-aggregated live total. Preserve all
    // canonical usage fields, including completeness and provider totals.
    receipt.tokens_spent = tokens_spent;

    if !earns_active(&receipt, entry.capabilities) {
        let probes = ProbeId::ALL
            .into_iter()
            .filter(|probe| probe.is_required_for(entry.capabilities))
            .filter(|probe| {
                !receipt
                    .result(*probe)
                    .is_some_and(|result| result.outcome.is_pass())
            })
            .collect();
        return Err(GenesisError::ActiveNotEarned { probes });
    }
    let canonical_receipt = to_jcs_bytes(&receipt)?;

    Ok(GenesisOutput {
        receipt,
        canonical_receipt,
    })
}

fn ensure_receipt_expiry_representable(ran_at: Timestamp) -> Result<(), GenesisError> {
    timestamp_after(ran_at, RECEIPT_FRESHNESS_MS).map(|_| ())
}

fn timestamp_after(value: Timestamp, delta_ms: i64) -> Result<Timestamp, GenesisError> {
    let millis = value
        .unix_millis()
        .checked_add(delta_ms)
        .ok_or(GenesisError::TimeGateOutOfRange)?;
    Timestamp::from_unix_millis(millis).map_err(|_| GenesisError::TimeGateOutOfRange)
}

fn reviewed_stop_reason_map() -> Result<StopReasonMap, GenesisError> {
    Ok(StopReasonMap {
        end_turn: vec![bounded("stop_reason_map.end_turn", "stop")?],
        tool_use: vec![bounded("stop_reason_map.tool_use", "tool_calls")?],
        max_output_tokens: vec![bounded("stop_reason_map.max_output_tokens", "length")?],
        stop_sequence: Vec::new(),
        refusal: Vec::new(),
        failure: vec![
            StopFailureMapping {
                token: bounded("stop_reason_map.failure", "content_filter")?,
                kind: ProviderFailureKind::ContentFiltered,
            },
            StopFailureMapping {
                token: bounded("stop_reason_map.failure", "insufficient_system_resource")?,
                kind: ProviderFailureKind::InsufficientProviderResource,
            },
        ],
    })
}

fn reviewed_error_map() -> ErrorClassMap {
    ErrorClassMap {
        status: vec![
            StatusClassRule {
                status: 400,
                kind: ProviderFailureKind::InvalidRequest,
            },
            StatusClassRule {
                status: 401,
                kind: ProviderFailureKind::Authentication,
            },
            StatusClassRule {
                status: 402,
                kind: ProviderFailureKind::Billing,
            },
            StatusClassRule {
                status: 404,
                kind: ProviderFailureKind::ModelNotFound,
            },
            StatusClassRule {
                status: 422,
                kind: ProviderFailureKind::InvalidRequest,
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

fn bounded<const N: usize>(
    field: &'static str,
    value: &'static str,
) -> Result<BoundedString<N>, GenesisError> {
    BoundedString::new(value).map_err(|source| GenesisError::StaticPolicy { field, source })
}
