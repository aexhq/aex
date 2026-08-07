//! Live-test companion for the model-catalog component shipped in `brain-mux`.
//!
//! Primary live concerns: the real provider model qualification matrix plus disable and
//! change probes.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
//!
//! # Why the live `[[test]]` targets are not declared yet
//!
//! Every probe in [`plan`] needs a real customer-owned key for its provider,
//! and `00-orchestrator-conventions.md` OD-07 puts nothing credentialed in this
//! run. The manifest therefore keeps `not_applicable.targets` with that exact
//! structural reason. This package carries the provider-independent harness
//! and a protected publisher executable, but no command can claim a probe ran.
//! The async configured executor can verify typed P-01--P-23 evidence, while
//! provider-specific programs still require exact owner-supplied catalog,
//! request, key-custody and fault-harness inputs.
//!
//! The prerequisite path is the part that can be proved without a key, and it
//! is: [`ProviderKeys::require`] goes through
//! `aex_test_harness::env::require_first`,
//! so an absent key **panics with the variable's name**. There is no branch
//! that can return early and report green.

use std::collections::BTreeMap;

use aex_model_catalog::canonical::NormalizedUsage;
use aex_model_catalog::document::{AdapterSourceDigest, CapabilitySet, EntryState, ModelEntry};
use aex_model_catalog::primitives::{BoundedString, ModelSlug};
use aex_model_catalog::receipt::{
    ConformanceReceipt, ObservedFact, PlaneId, ProbeId, ProbeOutcome, ProbeResult,
    ProbeSuiteRevision, RECEIPT_FRESHNESS_MS, Region,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, Uuid7};

pub mod catalog_source;
pub mod evidence;
pub mod executor;
pub mod publisher;

/// The canonical environment variable carrying a provider's live key.
///
/// One per provider, named after the provider's own wire spelling so a reader
/// can never wire the wrong key to the wrong adapter.
#[must_use]
pub const fn key_variable(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Openai => "AEX_LIVE_PROVIDER_KEY_OPENAI",
        ProviderId::Anthropic => "AEX_LIVE_PROVIDER_KEY_ANTHROPIC",
        ProviderId::Deepseek => "AEX_LIVE_PROVIDER_KEY_DEEPSEEK",
        ProviderId::Zai => "AEX_LIVE_PROVIDER_KEY_ZAI",
        ProviderId::Moonshotai => "AEX_LIVE_PROVIDER_KEY_MOONSHOTAI",
        ProviderId::Google => "AEX_LIVE_PROVIDER_KEY_GOOGLE",
        ProviderId::Openrouter => "AEX_LIVE_PROVIDER_KEY_OPENROUTER",
        ProviderId::VercelAiGateway => "AEX_LIVE_PROVIDER_KEY_VERCEL_AI_GATEWAY",
    }
}

/// Accepted live-key variables in precedence order.
///
/// The provider-qualified AEX name is always authoritative. Anthropic and
/// `DeepSeek` additionally accept the legacy names documented by this
/// workspace. No alias is guessed for the other providers.
#[must_use]
pub const fn key_variables(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Openai => &["AEX_LIVE_PROVIDER_KEY_OPENAI"],
        ProviderId::Anthropic => &["AEX_LIVE_PROVIDER_KEY_ANTHROPIC", "ANTHROPIC_API_KEY"],
        ProviderId::Deepseek => &["AEX_LIVE_PROVIDER_KEY_DEEPSEEK", "DEEPSEEK_API_KEY"],
        ProviderId::Zai => &["AEX_LIVE_PROVIDER_KEY_ZAI"],
        ProviderId::Moonshotai => &["AEX_LIVE_PROVIDER_KEY_MOONSHOTAI"],
        ProviderId::Google => &["AEX_LIVE_PROVIDER_KEY_GOOGLE"],
        ProviderId::Openrouter => &["AEX_LIVE_PROVIDER_KEY_OPENROUTER"],
        ProviderId::VercelAiGateway => &["AEX_LIVE_PROVIDER_KEY_VERCEL_AI_GATEWAY"],
    }
}

/// Resolves the live keys the probe suite needs.
#[derive(Debug, Default)]
pub struct ProviderKeys;

impl ProviderKeys {
    /// Reads one provider's key.
    ///
    /// # Panics
    ///
    /// Panics, naming the variable, when the key is absent or empty. That is
    /// the whole point: a live probe with no credential is a failure, never a
    /// skip.
    #[must_use]
    pub fn require(provider: ProviderId) -> String {
        Self::require_named(provider).1
    }

    /// Reads one provider's key and returns the accepted variable that supplied
    /// it.
    ///
    /// The name is safe to retain in qualification metadata; the plaintext is
    /// not. The authoritative name still wins over a documented legacy alias.
    ///
    /// # Panics
    ///
    /// Panics under the same conditions as [`Self::require`].
    #[must_use]
    pub fn require_named(provider: ProviderId) -> (&'static str, String) {
        aex_test_harness::env::require_first_named(key_variables(provider))
    }
}

/// The probe suite this package runs, and what each probe proves.
pub mod plan {
    use super::{ProbeId, ProviderId};

    /// One row of the conformance matrix.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ProbePlan {
        /// Which probe.
        pub probe: ProbeId,
        /// What a passing run establishes.
        pub proves: &'static str,
    }

    /// The twenty-three probes, in id order.
    ///
    /// Derived from `ProbeId::ALL` rather than hand-maintained, so a probe
    /// added to the catalog crate cannot be silently unrun here.
    #[must_use]
    pub fn all() -> Vec<ProbePlan> {
        ProbeId::ALL
            .into_iter()
            .map(|probe| ProbePlan {
                probe,
                proves: proves(probe),
            })
            .collect()
    }

    /// What each probe establishes, for the receipt's own record.
    #[must_use]
    pub const fn proves(probe: ProbeId) -> &'static str {
        match probe {
            ProbeId::P01 => "minimal text; the streamed seal matches the non-streamed shape",
            ProbeId::P02 => "a long stream stays ordered with no coalescing loss",
            ProbeId::P03 => "the system instruction is honoured",
            ProbeId::P04 => "one tool call whose arguments parse to the declared schema",
            ProbeId::P05 => "parallel tool calls carry distinct ids",
            ProbeId::P06 => "a tool-result round trip produces a second assistant turn",
            ProbeId::P07 => "every declared tool-choice mode behaves as declared",
            ProbeId::P08 => "structured output in the declared encoding validates",
            ProbeId::P09 => "reasoning produces reasoning content and reasoning tokens",
            ProbeId::P10 => "reasoning replay is accepted and omitting it is rejected",
            ProbeId::P11 => "usage is complete and matches the declared mapping",
            ProbeId::P12 => "an identical long prefix produces non-zero cache-read tokens",
            ProbeId::P13 => "the output ceiling produces MaxOutputTokens",
            ProbeId::P14 => "a stop sequence produces StopSequence",
            ProbeId::P15 => "mid-stream cancellation is typed Cancelled with bytes recorded",
            ProbeId::P16 => "long context at 80 percent of the declared window completes",
            ProbeId::P17 => "context window plus one is rejected, not truncated",
            ProbeId::P18 => "a forced drop at four points yields the exact dispatch proof",
            ProbeId::P19 => "rate-limit feedback is observed, or its absence is recorded",
            ProbeId::P20 => "the error taxonomy maps as declared and the body shape is recorded",
            ProbeId::P21 => "the credential appears in no response-derived artefact",
            ProbeId::P22 => "an oversized frame is rejected typed",
            ProbeId::P23 => "an idle stream is a typed timeout, not a hang",
        }
    }

    /// The providers a full matrix run covers.
    #[must_use]
    pub fn providers() -> Vec<ProviderId> {
        ProviderId::ALL.to_vec()
    }
}

/// A probe run's verdict, before it becomes a receipt row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRun {
    /// Which probe ran.
    pub probe: ProbeId,
    /// What happened.
    pub outcome: ProbeOutcome,
    /// Facts the run turned from documentation ambiguity into a record.
    pub observed: Vec<ObservedFact>,
    /// How long it took.
    pub duration_ms: u32,
}

/// Assembles a [`ConformanceReceipt`] from a completed matrix run.
///
/// Deliberately incapable of producing a receipt that would promote an unproved
/// entry: [`ReceiptBuilder::build`] requires a run for **every** probe, and a
/// missing run is an error rather than an implied pass.
#[derive(Debug)]
pub struct ReceiptBuilder {
    adapter_source: AdapterSourceDigest,
    suite: ProbeSuiteRevision,
    plane: PlaneId,
    region: Region,
    ran_at: Timestamp,
    runs: BTreeMap<ProbeId, ProbeRun>,
    request_ids: Vec<BoundedString<80>>,
    tokens_spent: NormalizedUsage,
}

/// Why a receipt could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// A probe produced no run at all.
    #[error("the matrix produced no run for {probe:?}; an unrun probe is not a pass")]
    MissingRun {
        /// Which probe.
        probe: ProbeId,
    },
    /// The receipt's expiry would fall outside the wire timestamp range.
    #[error("the receipt expiry is outside the representable range")]
    ExpiryOutOfRange,
}

impl ReceiptBuilder {
    /// Starts a receipt for one `(provider, model)` pair.
    #[must_use]
    pub fn new(
        adapter_source: AdapterSourceDigest,
        suite: ProbeSuiteRevision,
        plane: PlaneId,
        region: Region,
        ran_at: Timestamp,
    ) -> Self {
        Self {
            adapter_source,
            suite,
            plane,
            region,
            ran_at,
            runs: BTreeMap::new(),
            request_ids: Vec::new(),
            tokens_spent: NormalizedUsage::default(),
        }
    }

    /// Records one probe run. A second run for the same probe replaces the
    /// first, so a retry is visible as one row rather than two.
    pub fn record(&mut self, run: ProbeRun) {
        self.runs.insert(run.probe, run);
    }

    /// Records a provider request id observed during the run.
    pub fn observe_request_id(&mut self, id: &str) {
        self.request_ids.push(BoundedString::truncating(id));
    }

    /// Adds the tokens a probe spent. A zero-dollar `BYOK` fact.
    pub fn spend(&mut self, usage: NormalizedUsage) {
        self.tokens_spent.input_tokens = self
            .tokens_spent
            .input_tokens
            .saturating_add(usage.input_tokens);
        self.tokens_spent.output_tokens = self
            .tokens_spent
            .output_tokens
            .saturating_add(usage.output_tokens);
        self.tokens_spent.reasoning_tokens = self
            .tokens_spent
            .reasoning_tokens
            .saturating_add(usage.reasoning_tokens);
        self.tokens_spent.cache_read_input_tokens = self
            .tokens_spent
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        self.tokens_spent.cache_write_input_tokens = self
            .tokens_spent
            .cache_write_input_tokens
            .saturating_add(usage.cache_write_input_tokens);
    }

    /// Assembles the receipt.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError::MissingRun`] when any probe produced no run, and
    /// [`ReceiptError::ExpiryOutOfRange`] when the freshness window cannot be
    /// expressed.
    pub fn build(
        self,
        entry_digest: ContentHash,
        evidence_digest: ContentHash,
        receipt_id: Uuid7,
    ) -> Result<ConformanceReceipt, ReceiptError> {
        let mut results = Vec::with_capacity(ProbeId::ALL.len());
        for probe in ProbeId::ALL {
            let run = self
                .runs
                .get(&probe)
                .ok_or(ReceiptError::MissingRun { probe })?;
            results.push(ProbeResult {
                probe,
                outcome: run.outcome.clone(),
                observed: run.observed.clone(),
                duration_ms: run.duration_ms,
            });
        }
        let expires_at =
            Timestamp::from_unix_millis(self.ran_at.unix_millis() + RECEIPT_FRESHNESS_MS)
                .map_err(|_| ReceiptError::ExpiryOutOfRange)?;
        Ok(ConformanceReceipt {
            receipt_id,
            probe_suite_revision: self.suite,
            adapter_source: self.adapter_source,
            catalog_entry_digest: entry_digest,
            ran_at: self.ran_at,
            expires_at,
            plane: self.plane,
            region: self.region,
            results,
            provider_request_ids: self.request_ids,
            tokens_spent: self.tokens_spent,
            evidence_digest,
        })
    }
}

/// Whether a receipt earns `Active` for the capabilities an entry declares.
///
/// The catalog crate enforces this at load; the publishing tool needs the same
/// answer *before* it writes a document, so the rule is asked here rather than
/// discovered by a failed load.
#[must_use]
pub fn earns_active(receipt: &ConformanceReceipt, capabilities: CapabilitySet) -> bool {
    ProbeId::ALL.into_iter().all(|probe| {
        !probe.is_required_for(capabilities)
            || receipt
                .result(probe)
                .is_some_and(|result| result.outcome.is_pass())
    })
}

/// One row of a staging diff between a provider's model list and the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagingChange {
    /// The provider lists a model the catalog does not carry.
    Added {
        /// Which provider.
        provider: ProviderId,
        /// Which model.
        model: ModelSlug,
    },
    /// The catalog carries a model the provider no longer lists.
    Withdrawn {
        /// Which provider.
        provider: ProviderId,
        /// Which model.
        model: ModelSlug,
    },
}

/// Diffs a provider's live model list against the catalog's entries.
///
/// Every addition is staged, never activated: a model list is a name, and a
/// name is not evidence (OD-24). The caller must run the probe suite and embed
/// a passing receipt before an entry can become `Active`.
#[must_use]
pub fn stage(
    provider: ProviderId,
    listed: &[ModelSlug],
    catalog_entries: &[ModelEntry],
) -> Vec<StagingChange> {
    let known: Vec<&ModelSlug> = catalog_entries
        .iter()
        .filter(|entry| entry.provider == provider)
        .map(|entry| &entry.model)
        .collect();

    let mut changes = Vec::new();
    for model in listed {
        if !known.contains(&model) {
            changes.push(StagingChange::Added {
                provider,
                model: model.clone(),
            });
        }
    }
    for model in known {
        if !listed.contains(model) {
            changes.push(StagingChange::Withdrawn {
                provider,
                model: model.clone(),
            });
        }
    }
    changes
}

/// The state a staged entry enters. Always [`EntryState::Staged`]: there is no
/// path from a model list to `Active`.
#[must_use]
pub const fn staging_state() -> EntryState {
    EntryState::Staged
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::NormalizedUsage;
    use aex_model_catalog::document::{Capability, CapabilitySet, EntryState};
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::ModelSlug;
    use aex_model_catalog::receipt::{PlaneId, ProbeId, ProbeOutcome, ProbeSuiteRevision, Region};
    use aex_wire::provider::ProviderId;
    use aex_wire::{ContentHash, Uuid7};

    use super::{
        ProbeRun, ReceiptBuilder, ReceiptError, StagingChange, earns_active, key_variable,
        key_variables, plan, stage, staging_state,
    };

    fn builder() -> ReceiptBuilder {
        ReceiptBuilder::new(
            fixture::adapter("live-adapter"),
            ProbeSuiteRevision(1),
            PlaneId(fixture::bounded("dev")),
            Region(fixture::bounded("eu-west-1")),
            fixture::at(1_800_000_000_000),
        )
    }

    fn pass(probe: ProbeId) -> ProbeRun {
        ProbeRun {
            probe,
            outcome: ProbeOutcome::Pass,
            observed: Vec::new(),
            duration_ms: 5,
        }
    }

    #[test]
    #[should_panic(expected = "`AEX_LIVE_PROVIDER_KEY_ANTHROPIC`, `ANTHROPIC_API_KEY` are absent")]
    fn an_absent_provider_key_fails_loudly_rather_than_skipping() {
        // The whole prerequisite contract in one case: there is no branch that
        // can observe an absent key and report green.
        let _ = aex_test_harness::env::classify_first(
            key_variables(ProviderId::Anthropic)
                .iter()
                .map(|name| (*name, Err(std::env::VarError::NotPresent))),
        );
    }

    #[test]
    fn every_provider_has_its_own_key_variable() {
        let mut seen = Vec::new();
        for provider in ProviderId::ALL {
            let variable = key_variable(*provider);
            assert_eq!(key_variables(*provider)[0], variable);
            assert!(
                variable.starts_with("AEX_LIVE_PROVIDER_KEY_"),
                "{variable} is not in the reserved namespace"
            );
            assert!(!seen.contains(&variable), "{variable} is shared");
            seen.push(variable);
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn only_documented_legacy_provider_key_names_are_accepted() {
        assert_eq!(
            key_variables(ProviderId::Anthropic),
            &["AEX_LIVE_PROVIDER_KEY_ANTHROPIC", "ANTHROPIC_API_KEY"]
        );
        assert_eq!(
            key_variables(ProviderId::Deepseek),
            &["AEX_LIVE_PROVIDER_KEY_DEEPSEEK", "DEEPSEEK_API_KEY"]
        );
        for provider in [
            ProviderId::Openai,
            ProviderId::Zai,
            ProviderId::Moonshotai,
            ProviderId::Google,
            ProviderId::Openrouter,
            ProviderId::VercelAiGateway,
        ] {
            assert_eq!(key_variables(provider), &[key_variable(provider)]);
        }
    }

    #[test]
    fn a_documented_legacy_name_can_satisfy_the_live_key_prerequisite() {
        let value =
            aex_test_harness::env::classify_first(key_variables(ProviderId::Deepseek).iter().map(
                |name| match *name {
                    "DEEPSEEK_API_KEY" => (*name, Ok("present".to_owned())),
                    _ => (*name, Err(std::env::VarError::NotPresent)),
                },
            ));
        assert_eq!(value, "present");
    }

    #[test]
    fn the_probe_plan_is_derived_from_the_catalog_registry() {
        let planned = plan::all();
        assert_eq!(planned.len(), ProbeId::ALL.len());
        for (row, probe) in planned.iter().zip(ProbeId::ALL) {
            assert_eq!(row.probe, probe);
            assert!(!row.proves.is_empty(), "{probe:?} states nothing");
        }
        assert_eq!(plan::providers().len(), 8);
    }

    #[test]
    fn a_missing_run_is_an_error_not_an_implied_pass() {
        let mut builder = builder();
        for probe in ProbeId::ALL {
            if probe == ProbeId::P15 {
                continue;
            }
            builder.record(pass(probe));
        }
        assert_eq!(
            builder
                .build(
                    ContentHash::of(b"entry"),
                    ContentHash::of(b"evidence"),
                    Uuid7::compose(1, [1; 10])
                )
                .expect_err("an unrun probe cannot be assembled away"),
            ReceiptError::MissingRun {
                probe: ProbeId::P15
            }
        );
    }

    #[test]
    fn a_complete_run_assembles_a_receipt_with_every_row() {
        let mut builder = builder();
        for probe in ProbeId::ALL {
            builder.record(pass(probe));
        }
        builder.observe_request_id("req_abc");
        builder.spend(NormalizedUsage {
            input_tokens: 10,
            output_tokens: 20,
            reasoning_tokens: 5,
            ..NormalizedUsage::default()
        });
        let receipt = builder
            .build(
                ContentHash::of(b"entry"),
                ContentHash::of(b"evidence"),
                Uuid7::compose(1, [1; 10]),
            )
            .expect("a complete run assembles");
        assert_eq!(receipt.results.len(), ProbeId::ALL.len());
        assert_eq!(receipt.tokens_spent.output_tokens, 20);
        assert!(receipt.expires_at > receipt.ran_at);
        assert_eq!(receipt.provider_request_ids.len(), 1);
    }

    #[test]
    fn a_receipt_with_any_failing_unconditional_probe_does_not_earn_active() {
        let mut builder = builder();
        for probe in ProbeId::ALL {
            builder.record(pass(probe));
        }
        builder.record(ProbeRun {
            probe: ProbeId::P20,
            outcome: ProbeOutcome::Fail {
                detail: fixture::bounded("the 404 mapped to the wrong class"),
            },
            observed: Vec::new(),
            duration_ms: 5,
        });
        let receipt = builder
            .build(
                ContentHash::of(b"entry"),
                ContentHash::of(b"evidence"),
                Uuid7::compose(1, [1; 10]),
            )
            .expect("assembles");
        assert!(
            !earns_active(&receipt, CapabilitySet::EMPTY),
            "P-20 is unconditional, so a failure must block Active"
        );
    }

    #[test]
    fn a_capability_probe_only_matters_when_the_capability_is_declared() {
        let mut builder = builder();
        for probe in ProbeId::ALL {
            builder.record(pass(probe));
        }
        builder.record(ProbeRun {
            probe: ProbeId::P04,
            outcome: ProbeOutcome::NotApplicable {
                capability: Capability::Tools,
            },
            observed: Vec::new(),
            duration_ms: 0,
        });
        let receipt = builder
            .build(
                ContentHash::of(b"entry"),
                ContentHash::of(b"evidence"),
                Uuid7::compose(1, [1; 10]),
            )
            .expect("assembles");
        assert!(earns_active(&receipt, CapabilitySet::EMPTY));
        assert!(!earns_active(
            &receipt,
            CapabilitySet::from_slice(&[Capability::Tools])
        ));
    }

    #[test]
    fn staging_reports_additions_and_withdrawals_and_promotes_nothing() {
        let entries = vec![
            fixture::entry(
                ProviderId::Deepseek,
                "deepseek-v4-pro",
                CapabilitySet::EMPTY,
            ),
            fixture::entry(ProviderId::Deepseek, "deepseek-v3", CapabilitySet::EMPTY),
            fixture::entry(ProviderId::Openai, "gpt-5.2", CapabilitySet::EMPTY),
        ];
        let listed = vec![
            ModelSlug::new("deepseek-v4-pro").expect("slug"),
            ModelSlug::new("deepseek-v4-flash").expect("slug"),
        ];
        let changes = stage(ProviderId::Deepseek, &listed, &entries);
        assert_eq!(
            changes,
            vec![
                StagingChange::Added {
                    provider: ProviderId::Deepseek,
                    model: ModelSlug::new("deepseek-v4-flash").expect("slug"),
                },
                StagingChange::Withdrawn {
                    provider: ProviderId::Deepseek,
                    model: ModelSlug::new("deepseek-v3").expect("slug"),
                },
            ]
        );
        assert_eq!(staging_state(), EntryState::Staged);
    }

    #[test]
    fn staging_ignores_another_providers_entries() {
        let entries = vec![fixture::entry(
            ProviderId::Openai,
            "gpt-5.2",
            CapabilitySet::EMPTY,
        )];
        let listed = vec![ModelSlug::new("claude-opus-5").expect("slug")];
        let changes = stage(ProviderId::Anthropic, &listed, &entries);
        assert_eq!(
            changes,
            vec![StagingChange::Added {
                provider: ProviderId::Anthropic,
                model: ModelSlug::new("claude-opus-5").expect("slug"),
            }]
        );
    }
}
