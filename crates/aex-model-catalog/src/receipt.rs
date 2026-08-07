//! The conformance receipt and its probe registry (plan 08 §8).
//!
//! A receipt turns a documentation claim into an observed fact. The gate is a
//! **document load invariant** (D-05): an `Active` entry whose declared
//! capabilities are not all covered by passing probes, or whose receipt was
//! earned against a different adapter source tree, cannot be loaded at all.

use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, Uuid7};
use serde::{Deserialize, Serialize};

use crate::canonical::NormalizedUsage;
use crate::document::{AdapterSourceDigest, Capability};
use crate::primitives::BoundedString;

/// The Unix epoch. Checked at compile time, so no runtime unwrap exists.
pub(crate) const EPOCH: Timestamp = match Timestamp::from_unix_millis(0) {
    Ok(value) => value,
    Err(_) => panic!("the Unix epoch is inside the wire timestamp range"),
};

/// Which revision of the probe suite a receipt was earned against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProbeSuiteRevision(pub u16);

/// A deployment plane identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlaneId(pub BoundedString<16>);

/// An AWS region identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Region(pub BoundedString<32>);

/// The twenty-three conformance probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeId {
    /// P-01 minimal text; the streamed seal matches the non-streamed shape.
    P01,
    /// P-02 long stream: at least 8 KiB over at least 200 frames, ordered.
    P02,
    /// P-03 the system instruction is honoured.
    P03,
    /// P-04 one tool call whose arguments parse to the declared schema.
    P04,
    /// P-05 parallel tool calls with distinct ids.
    P05,
    /// P-06 a tool-result round trip produces a second assistant turn.
    P06,
    /// P-07 every declared `tool_choice` mode behaves as declared.
    P07,
    /// P-08 structured output in the declared encoding validates.
    P08,
    /// P-09 reasoning produces reasoning content and reasoning tokens.
    P09,
    /// P-10 reasoning replay is accepted, and omitting it is rejected.
    P10,
    /// P-11 usage is complete and matches the declared mapping.
    P11,
    /// P-12 an identical long prefix produces non-zero cache-read tokens.
    P12,
    /// P-13 the output ceiling produces `MaxOutputTokens`.
    P13,
    /// P-14 a stop sequence produces `StopSequence`.
    P14,
    /// P-15 mid-stream cancellation is typed `Cancelled` with bytes recorded.
    P15,
    /// P-16 long context at 80 % of the declared window completes.
    P16,
    /// P-17 context window plus one is rejected, not truncated.
    P17,
    /// P-18 forced drop at four points yields the exact dispatch proof.
    P18,
    /// P-19 rate-limit feedback is observed, or its absence is recorded.
    P19,
    /// P-20 the error taxonomy maps as declared and the body shape is recorded.
    P20,
    /// P-21 the credential appears in no response-derived artefact.
    P21,
    /// P-22 an oversized frame is rejected typed.
    P22,
    /// P-23 an idle stream is a typed timeout, not a hang.
    P23,
}

impl ProbeId {
    /// Every probe, in id order. A receipt must carry a result for each.
    pub const ALL: [Self; 23] = [
        Self::P01,
        Self::P02,
        Self::P03,
        Self::P04,
        Self::P05,
        Self::P06,
        Self::P07,
        Self::P08,
        Self::P09,
        Self::P10,
        Self::P11,
        Self::P12,
        Self::P13,
        Self::P14,
        Self::P15,
        Self::P16,
        Self::P17,
        Self::P18,
        Self::P19,
        Self::P20,
        Self::P21,
        Self::P22,
        Self::P23,
    ];

    /// The wire token, for example `p-01`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P01 => "p-01",
            Self::P02 => "p-02",
            Self::P03 => "p-03",
            Self::P04 => "p-04",
            Self::P05 => "p-05",
            Self::P06 => "p-06",
            Self::P07 => "p-07",
            Self::P08 => "p-08",
            Self::P09 => "p-09",
            Self::P10 => "p-10",
            Self::P11 => "p-11",
            Self::P12 => "p-12",
            Self::P13 => "p-13",
            Self::P14 => "p-14",
            Self::P15 => "p-15",
            Self::P16 => "p-16",
            Self::P17 => "p-17",
            Self::P18 => "p-18",
            Self::P19 => "p-19",
            Self::P20 => "p-20",
            Self::P21 => "p-21",
            Self::P22 => "p-22",
            Self::P23 => "p-23",
        }
    }

    /// Whether the probe is required for **every** entry, whatever it declares.
    #[must_use]
    pub const fn is_unconditional(self) -> bool {
        matches!(
            self,
            Self::P01
                | Self::P02
                | Self::P11
                | Self::P13
                | Self::P15
                | Self::P16
                | Self::P17
                | Self::P18
                | Self::P19
                | Self::P20
                | Self::P21
                | Self::P22
                | Self::P23
        )
    }

    /// Which capability makes the probe required, where one does.
    #[must_use]
    pub const fn required_by(self) -> Option<Capability> {
        match self {
            Self::P03 => Some(Capability::SystemInstruction),
            Self::P04 | Self::P06 => Some(Capability::Tools),
            Self::P05 => Some(Capability::ParallelTools),
            Self::P08 => Some(Capability::StructuredOutput),
            Self::P09 => Some(Capability::Reasoning),
            Self::P10 => Some(Capability::ReasoningReplay),
            Self::P14 => Some(Capability::StopSequences),
            _ => None,
        }
    }

    /// Whether this probe is required for an entry declaring `capabilities`.
    ///
    /// `tool_choice` is special: P-07 is required when **any** of the three
    /// `ToolChoice*` capabilities is declared.
    #[must_use]
    pub fn is_required_for(self, capabilities: crate::document::CapabilitySet) -> bool {
        if self.is_unconditional() {
            return true;
        }
        if matches!(self, Self::P07) {
            return capabilities.has(Capability::ToolChoiceRequired)
                || capabilities.has(Capability::ToolChoiceNamed)
                || capabilities.has(Capability::ToolChoiceNone);
        }
        if matches!(self, Self::P12) {
            return capabilities.has(Capability::PromptCacheExplicit)
                || capabilities.has(Capability::PromptCacheImplicit);
        }
        self.required_by()
            .is_some_and(|capability| capabilities.has(capability))
    }

    /// Which probe proves a capability, for the `CapabilityWithoutProbe` load
    /// invariant. Capabilities with no dedicated probe are proved by the
    /// unconditional set.
    #[must_use]
    pub const fn proving(capability: Capability) -> Option<Self> {
        match capability {
            Capability::SystemInstruction => Some(Self::P03),
            Capability::Tools => Some(Self::P04),
            Capability::ParallelTools => Some(Self::P05),
            Capability::ToolChoiceRequired
            | Capability::ToolChoiceNamed
            | Capability::ToolChoiceNone => Some(Self::P07),
            Capability::StructuredOutput => Some(Self::P08),
            Capability::Reasoning => Some(Self::P09),
            Capability::ReasoningReplay => Some(Self::P10),
            Capability::PromptCacheExplicit | Capability::PromptCacheImplicit => Some(Self::P12),
            Capability::StopSequences => Some(Self::P14),
            _ => None,
        }
    }
}

/// A probe's verdict.
///
/// There is deliberately no `Skipped` member (D-26): a probe that cannot run is
/// a `Fail`, so `no-silent-skips` is expressible in the type rather than
/// enforced by review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeOutcome {
    /// The probe ran and the behaviour matched the declaration.
    Pass,
    /// The probe ran and the behaviour did not match, or it could not run.
    Fail {
        /// Bounded, redacted detail.
        detail: BoundedString<512>,
    },
    /// The entry does not declare the capability the probe exercises.
    NotApplicable {
        /// Which capability is absent.
        capability: Capability,
    },
}

impl ProbeOutcome {
    /// Whether the outcome permits an `Active` entry.
    #[must_use]
    pub const fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// A documentation ambiguity turned into a durable answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedFact {
    /// What was being decided, for example `deepseek.reasoning_effort.nesting`.
    pub key: BoundedString<64>,
    /// What the provider actually did.
    pub value: BoundedString<256>,
}

/// One probe's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    /// Which probe.
    pub probe: ProbeId,
    /// Its verdict.
    pub outcome: ProbeOutcome,
    /// Facts the probe recorded.
    pub observed: Vec<ObservedFact>,
    /// How long it took.
    pub duration_ms: u32,
}

/// The evidence that makes an entry admissible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceReceipt {
    /// A time-ordered receipt id.
    pub receipt_id: Uuid7,
    /// Which probe suite produced it.
    pub probe_suite_revision: ProbeSuiteRevision,
    /// Which adapter source tree produced it.
    pub adapter_source: AdapterSourceDigest,
    /// The digest of the entry this receipt sits inside, excluding the receipt.
    pub catalog_entry_digest: ContentHash,
    /// When the suite ran.
    pub ran_at: Timestamp,
    /// When the evidence goes stale. Freshness is seven days (Area 7).
    pub expires_at: Timestamp,
    /// Which plane ran it.
    pub plane: PlaneId,
    /// Which region ran it.
    pub region: Region,
    /// Sorted by [`ProbeId`], complete.
    pub results: Vec<ProbeResult>,
    /// The provider request ids observed, for provider-side correlation.
    pub provider_request_ids: Vec<BoundedString<80>>,
    /// What the suite cost in tokens. A zero-dollar BYOK fact.
    pub tokens_spent: NormalizedUsage,
    /// A hash over the full evidence bundle held outside the document.
    pub evidence_digest: ContentHash,
}

/// The freshness window for a receipt, in milliseconds. Seven days (Area 7).
pub const RECEIPT_FRESHNESS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

impl ConformanceReceipt {
    /// The result for a probe, if the receipt carries one.
    #[must_use]
    pub fn result(&self, probe: ProbeId) -> Option<&ProbeResult> {
        self.results.iter().find(|r| r.probe == probe)
    }

    /// An empty receipt: every probe recorded as a failure with the reason.
    ///
    /// This is the shape the launch catalog ships with, and it is exactly why
    /// every launch entry is `Staged` (OD-24). It is a positive record that the
    /// evidence has not been earned, never an absence.
    ///
    /// # Panics
    ///
    /// Never: `reason` is truncated to the bounded length rather than rejected.
    #[must_use]
    pub fn unearned(
        adapter_source: AdapterSourceDigest,
        entry_digest: ContentHash,
        suite: ProbeSuiteRevision,
        reason: &str,
    ) -> Self {
        let detail = BoundedString::<512>::truncating(reason);
        Self {
            receipt_id: Uuid7::compose(0, [0; 10]),
            probe_suite_revision: suite,
            adapter_source,
            catalog_entry_digest: entry_digest,
            ran_at: EPOCH,
            expires_at: EPOCH,
            plane: PlaneId(BoundedString::truncating("none")),
            region: Region(BoundedString::truncating("none")),
            results: ProbeId::ALL
                .into_iter()
                .map(|probe| ProbeResult {
                    probe,
                    outcome: ProbeOutcome::Fail {
                        detail: detail.clone(),
                    },
                    observed: Vec::new(),
                    duration_ms: 0,
                })
                .collect(),
            provider_request_ids: Vec::new(),
            tokens_spent: NormalizedUsage::default(),
            evidence_digest: ContentHash::of(b""),
        }
    }
}
