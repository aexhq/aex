//! Production genesis candidate and output-builder regression tests.

use aex_brain_provider_gateway::build_identity::adapter_source_digest;
use aex_live_model_catalog::ProbeRun;
use aex_live_model_catalog::genesis::{
    DEEPSEEK_FLASH_MODEL, DEEPSEEK_GENESIS_CAPABILITIES, GenesisError, GenesisMetadata,
    build_deepseek_genesis, deepseek_v4_flash_candidate,
};
use aex_model_catalog::canonical::{NormalizedUsage, UsageFieldSet};
use aex_model_catalog::catalog::catalog_entry_digest;
use aex_model_catalog::document::{
    CacheMode, CacheReadSemantics, Capability, CapabilitySet, Dialect, DurableOperationSupport,
    EndpointPin, EntryState, NamePattern, ReasoningEncoding, ReasoningMode, ReasoningReplay,
    SamplingSupport, StreamUsageDelivery, StructuredOutputPolicy, ToolArgumentEncoding,
    ToolEncoding,
};
use aex_model_catalog::failure::ProviderFailureKind;
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::receipt::{PlaneId, ProbeId, ProbeOutcome, Region};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, Uuid7, to_jcs_bytes};

const RAN_AT_MS: i64 = 1_800_000_000_000;

fn bounded<const N: usize>(value: &str) -> BoundedString<N> {
    BoundedString::new(value).expect("test value is bounded")
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("test instant is representable")
}

fn pass(probe: ProbeId) -> ProbeRun {
    ProbeRun {
        probe,
        outcome: ProbeOutcome::Pass,
        observed: Vec::new(),
        duration_ms: 5,
    }
}

fn complete_matrix() -> Vec<ProbeRun> {
    ProbeId::ALL.into_iter().map(pass).collect()
}

fn usage() -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: 1_000_000,
        cache_read_input_tokens: 64,
        cache_write_input_tokens: 0,
        output_tokens: 384_000,
        reasoning_tokens: 32_000,
        tool_use_prompt_tokens: 9,
        provider_total_tokens: Some(1_384_064),
        ..NormalizedUsage::default()
    }
}

fn metadata() -> GenesisMetadata {
    GenesisMetadata {
        receipt_id: Uuid7::compose(1, [7; 10]),
        plane: PlaneId(bounded("dev")),
        region: Region(bounded("eu-west-1")),
        ran_at: at(RAN_AT_MS),
        provider_request_ids: vec![bounded("req_deepseek_genesis")],
        tokens_spent: usage(),
        evidence_digest: ContentHash::of(b"immutable-live-evidence-bundle"),
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive assertion keeps the reviewed candidate policy auditable"
)]
fn reviewed_candidate_is_the_exact_shared_deepseek_policy() {
    let entry = deepseek_v4_flash_candidate().expect("build-stamped staged candidate");

    assert_eq!(entry.provider, ProviderId::Deepseek);
    assert_eq!(entry.model.as_str(), DEEPSEEK_FLASH_MODEL);
    assert_eq!(entry.state, EntryState::Staged);
    assert_eq!(entry.dialect, Dialect::DeepSeekChat);
    assert_eq!(entry.dialect_revision.0, 1);
    assert_eq!(entry.endpoint, EndpointPin::DeepSeekApi);
    assert_eq!(
        entry.capabilities,
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
        ])
    );
    assert_eq!(entry.capabilities, DEEPSEEK_GENESIS_CAPABILITIES);
    assert_eq!(entry.limits.context_window_tokens, 1_000_000);
    assert_eq!(entry.limits.max_output_tokens, 384_000);
    assert_eq!(entry.limits.min_output_tokens, 1);
    assert_eq!(entry.limits.max_reasoning_tokens, None);
    assert_eq!(entry.limits.min_reasoning_tokens, None);
    assert_eq!(entry.limits.max_tools, 128);
    assert_eq!(entry.limits.max_stop_sequences, 16);
    assert_eq!(entry.limits.temperature_milli, Some((0, 2_000)));
    assert_eq!(entry.limits.top_p_milli, Some((0, 1_000)));
    assert_eq!(entry.limits.min_cacheable_prefix_tokens, 64);
    assert_eq!(entry.limits.request_body_max_bytes, 32 * 1024 * 1024);
    assert_eq!(entry.limits.response_frame_max_bytes, 1024 * 1024);
    assert_eq!(entry.limits.stream_idle_timeout_ms, 60_000);
    assert_eq!(entry.limits.total_stream_deadline_ms, 900_000);
    assert_eq!(entry.sampling, SamplingSupport::Full);
    assert_eq!(entry.reasoning.mode, ReasoningMode::Optional);
    assert_eq!(
        entry.reasoning.encoding,
        ReasoningEncoding::DeepSeekThinking
    );
    assert_eq!(
        entry.reasoning.replay,
        ReasoningReplay::RequiredWithToolCalls
    );
    assert!(entry.reasoning.excludes_sampling);
    assert_eq!(
        entry.structured_output,
        StructuredOutputPolicy::JsonObjectOnly
    );
    assert_eq!(
        entry.tool_policy.encoding,
        ToolEncoding::OpenAiNestedFunction
    );
    assert_eq!(
        entry.tool_policy.arguments,
        ToolArgumentEncoding::JsonString
    );
    assert!(!entry.tool_policy.requires_stream_opt_in);
    assert_eq!(entry.tool_policy.max_name_bytes, 64);
    assert_eq!(
        entry.tool_policy.name_pattern,
        NamePattern::OpenAiFunctionName
    );
    assert_eq!(entry.cache_policy.mode, CacheMode::Implicit);
    assert_eq!(entry.cache_policy.max_breakpoints, 0);
    assert!(entry.usage_map.reasoning_included_in_output);
    assert_eq!(
        entry.usage_map.cache_read_field,
        CacheReadSemantics::HitMissSplit
    );
    assert_eq!(
        entry.usage_map.stream_usage_delivery,
        StreamUsageDelivery::TrailingChoicesEmptyChunk
    );
    assert_eq!(entry.usage_map.known_missing, UsageFieldSet::EMPTY);
    assert_eq!(entry.stop_reason_map.end_turn[0].as_str(), "stop");
    assert_eq!(entry.stop_reason_map.tool_use[0].as_str(), "tool_calls");
    assert_eq!(
        entry.stop_reason_map.max_output_tokens[0].as_str(),
        "length"
    );
    assert!(entry.stop_reason_map.stop_sequence.is_empty());
    assert!(entry.stop_reason_map.refusal.is_empty());
    assert_eq!(
        entry
            .stop_reason_map
            .failure
            .iter()
            .map(|row| (row.token.as_str(), row.kind))
            .collect::<Vec<_>>(),
        vec![
            ("content_filter", ProviderFailureKind::ContentFiltered),
            (
                "insufficient_system_resource",
                ProviderFailureKind::InsufficientProviderResource,
            ),
        ]
    );
    assert_eq!(
        entry
            .error_map
            .status
            .iter()
            .map(|row| (row.status, row.kind))
            .collect::<Vec<_>>(),
        vec![
            (400, ProviderFailureKind::InvalidRequest),
            (401, ProviderFailureKind::Authentication),
            (402, ProviderFailureKind::Billing),
            (404, ProviderFailureKind::ModelNotFound),
            (422, ProviderFailureKind::InvalidRequest),
            (429, ProviderFailureKind::RateLimited),
            (503, ProviderFailureKind::Overloaded),
        ]
    );
    assert!(entry.error_map.codes.is_empty());
    assert_eq!(
        entry.error_map.default_kind,
        ProviderFailureKind::ServerError
    );
    assert_eq!(entry.retry_policy.max_attempts, 3);
    assert_eq!(entry.retry_policy.in_call_status_retry, vec![429, 503]);
    assert_eq!(entry.durable_operation, DurableOperationSupport::None);
    assert_eq!(entry.concurrency_hint, 2_500);
    assert_eq!(entry.pricing_context, None);
}

#[test]
fn complete_live_matrix_becomes_standalone_canonical_assurance() {
    let metadata = metadata();
    let output = build_deepseek_genesis(complete_matrix(), metadata.clone())
        .expect("complete passing evidence earns assurance");
    let adapter = adapter_source_digest().expect("current adapter digest");
    assert_eq!(output.receipt.adapter_source, adapter);
    assert_eq!(output.receipt.tokens_spent, metadata.tokens_spent);
    assert_eq!(
        output.receipt.provider_request_ids,
        metadata.provider_request_ids
    );
    let mut active = deepseek_v4_flash_candidate().expect("reviewed candidate");
    active.state = EntryState::Active;
    assert_eq!(
        output.receipt.catalog_entry_digest,
        catalog_entry_digest(&active).expect("exact Active compatibility-entry digest")
    );
    assert_eq!(
        output.canonical_receipt,
        to_jcs_bytes(&output.receipt).expect("canonical receipt")
    );
}

#[test]
fn duplicate_probe_is_rejected_instead_of_silently_replaced() {
    let mut runs = complete_matrix();
    runs.push(pass(ProbeId::P20));
    assert!(matches!(
        build_deepseek_genesis(runs, metadata()),
        Err(GenesisError::DuplicateProbe {
            probe: ProbeId::P20
        })
    ));
}

#[test]
fn required_probe_failure_cannot_emit_an_active_document() {
    let mut runs = complete_matrix();
    runs[ProbeId::ALL
        .iter()
        .position(|probe| *probe == ProbeId::P20)
        .expect("P-20 exists")] = ProbeRun {
        probe: ProbeId::P20,
        outcome: ProbeOutcome::Fail {
            detail: bounded("taxonomy did not match the reviewed candidate"),
        },
        observed: Vec::new(),
        duration_ms: 5,
    };
    assert!(matches!(
        build_deepseek_genesis(runs, metadata()),
        Err(GenesisError::ActiveNotEarned { probes }) if probes == vec![ProbeId::P20]
    ));
}
