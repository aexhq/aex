//! Provider/model qualification scaffolding.
//!
//! This module is deliberately a *readiness* boundary, not a live-result
//! generator. It gives the protected publisher one closed registry of model
//! candidates, credential names, and declared capabilities, then exposes a
//! matrix with one slot for every `P-01` through `P-23` probe. The actual
//! provider calls still belong in a protected live runner. Until that runner
//! is supplied, [`PendingProbeExecutor`] fails every required probe and no
//! receipt row can be invented.

use aex_model_catalog::document::{Capability, CapabilitySet};
use aex_model_catalog::primitives::ModelSlug;
use aex_model_catalog::receipt::{ProbeId, ProbeOutcome};
use aex_wire::provider::ProviderId;

use crate::{ProbeRun, ProviderKeys, key_variables};

/// Provider authority and capability facts used by the future live matrix.
///
/// Every provider requires the protected run to supply an exact model slug.
/// The registry intentionally does not mint a model-list claim: a gateway
/// model is customer-selected, and even a native provider model must be bound
/// to the exact run and resulting receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderProfile {
    /// The customer-owned provider authority.
    pub provider: ProviderId,
    /// Whether the protected run must supply an exact model slug.
    pub requires_explicit_model: bool,
    /// Capability ceiling used to reject an accidental over-claim before I/O.
    pub capability_ceiling: CapabilitySet,
    /// Accepted credential variables, in precedence order.
    pub credential_variables: &'static [&'static str],
}

const TEXT_STREAM: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
]);

const OPENAI_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::SystemInstruction,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::ToolChoiceRequired,
    Capability::ToolChoiceNamed,
    Capability::ToolChoiceNone,
    Capability::StrictToolSchema,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::ReasoningReplay,
    Capability::PromptCacheImplicit,
    Capability::Temperature,
    Capability::TopP,
]);

const ANTHROPIC_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::ToolChoiceRequired,
    Capability::ToolChoiceNamed,
    Capability::ToolChoiceNone,
    Capability::StrictToolSchema,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::ReasoningReplay,
    Capability::PromptCacheExplicit,
    Capability::StopSequences,
    Capability::Temperature,
    Capability::TopP,
    Capability::SystemInstruction,
]);

const DEEPSEEK_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::ToolChoiceRequired,
    Capability::ToolChoiceNamed,
    Capability::ToolChoiceNone,
    Capability::StrictToolSchema,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::ReasoningReplay,
    Capability::PromptCacheImplicit,
    Capability::StopSequences,
    Capability::Temperature,
    Capability::TopP,
    Capability::SystemInstruction,
]);

const ZAI_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::ReasoningReplay,
    Capability::PromptCacheImplicit,
    Capability::StopSequences,
    Capability::Temperature,
    Capability::TopP,
    Capability::SystemInstruction,
]);

const MOONSHOT_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::ToolChoiceRequired,
    Capability::ToolChoiceNamed,
    Capability::ToolChoiceNone,
    Capability::StrictToolSchema,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::StopSequences,
    Capability::Temperature,
    Capability::TopP,
    Capability::SystemInstruction,
]);

const GOOGLE_CAPABILITIES: CapabilitySet = CapabilitySet::from_slice(&[
    Capability::TextIn,
    Capability::TextOut,
    Capability::Streaming,
    Capability::Tools,
    Capability::ParallelTools,
    Capability::ToolChoiceRequired,
    Capability::ToolChoiceNamed,
    Capability::ToolChoiceNone,
    Capability::StrictToolSchema,
    Capability::StructuredOutput,
    Capability::Reasoning,
    Capability::ReasoningReplay,
    Capability::PromptCacheImplicit,
    Capability::StopSequences,
    Capability::Temperature,
    Capability::TopP,
    Capability::SystemInstruction,
]);

/// Returns the closed profile for one provider authority.
#[must_use]
pub const fn profile(provider: ProviderId) -> ProviderProfile {
    let capability_ceiling = match provider {
        ProviderId::Openai => OPENAI_CAPABILITIES,
        ProviderId::Anthropic => ANTHROPIC_CAPABILITIES,
        ProviderId::Deepseek => DEEPSEEK_CAPABILITIES,
        ProviderId::Zai => ZAI_CAPABILITIES,
        ProviderId::Moonshotai => MOONSHOT_CAPABILITIES,
        ProviderId::Google => GOOGLE_CAPABILITIES,
        // These are adapter authorities, not model catalog claims. The
        // gateway's exact model and observed capabilities must be supplied by
        // the protected run; using an invented default would mint authority.
        ProviderId::Openrouter | ProviderId::VercelAiGateway => TEXT_STREAM,
    };
    ProviderProfile {
        provider,
        requires_explicit_model: true,
        capability_ceiling,
        credential_variables: key_variables(provider),
    }
}

/// A concrete provider/model target proposed for qualification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationTarget {
    /// Customer-owned provider authority.
    pub provider: ProviderId,
    /// Exact provider-native model slug.
    pub model: ModelSlug,
    /// Capabilities claimed for this exact pair, pending live proof.
    pub capabilities: CapabilitySet,
}

/// Why a target could not be admitted to a probe matrix.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadinessError {
    /// A model slug was empty or malformed.
    #[error("provider `{provider}` target model is invalid: {detail}")]
    InvalidModel {
        /// Provider authority.
        provider: ProviderId,
        /// Bounded parser detail.
        detail: String,
    },
    /// A target omitted one of the capabilities needed by every probe run.
    #[error("provider `{provider}` target omits required baseline capability `{capability:?}`")]
    MissingCapability {
        /// Provider authority.
        provider: ProviderId,
        /// Baseline capability.
        capability: Capability,
    },
    /// A target contains a capability bit this binary does not understand.
    #[error("provider `{provider}` target contains unknown capability bits 0x{bits:08x}")]
    UnknownCapabilityBits {
        /// Provider authority.
        provider: ProviderId,
        /// Unknown raw bits.
        bits: u32,
    },
    /// A target declared a capability outside the adapter's known ceiling.
    #[error("provider `{provider}` target declares unsupported capability `{capability:?}`")]
    UnsupportedCapability {
        /// Provider authority.
        provider: ProviderId,
        /// Capability outside the closed ceiling.
        capability: Capability,
    },
    /// No accepted customer credential variable was present.
    #[error("provider `{provider}` needs one non-empty credential from {variables:?}")]
    MissingCredential {
        /// Provider authority.
        provider: ProviderId,
        /// Accepted variables, in precedence order; values are never carried.
        variables: &'static [&'static str],
    },
    /// The future live runner has no implementation for a required probe.
    #[error("probe {probe:?} has no provider executor; no receipt row may be emitted")]
    ExecutorUnavailable {
        /// Probe with no implementation.
        probe: ProbeId,
    },
}

impl QualificationTarget {
    /// Validates an exact target against the closed provider registry.
    ///
    /// # Errors
    ///
    /// Rejects malformed slugs, missing text/streaming baseline capabilities,
    /// unknown capability bits and capabilities outside the provider adapter
    /// ceiling.
    pub fn new(
        provider: ProviderId,
        model: &str,
        capabilities: CapabilitySet,
    ) -> Result<Self, ReadinessError> {
        let model = ModelSlug::new(model).map_err(|error| ReadinessError::InvalidModel {
            provider,
            detail: error.to_string(),
        })?;
        let profile = profile(provider);
        if capabilities.has_unknown_bits() {
            let known = Capability::ALL.into_iter().fold(0_u32, |bits, capability| {
                bits | CapabilitySet::from_slice(&[capability]).0
            });
            return Err(ReadinessError::UnknownCapabilityBits {
                provider,
                bits: capabilities.0 & !known,
            });
        }
        for capability in [
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
        ] {
            if !capabilities.has(capability) {
                return Err(ReadinessError::MissingCapability {
                    provider,
                    capability,
                });
            }
        }
        for capability in capabilities.declared() {
            if !profile.capability_ceiling.has(capability) {
                return Err(ReadinessError::UnsupportedCapability {
                    provider,
                    capability,
                });
            }
        }
        Ok(Self {
            provider,
            model,
            capabilities,
        })
    }
}

/// One closed slot in the P-01–P-23 matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeSlot {
    /// Probe identity.
    pub probe: ProbeId,
    /// Whether a live provider call is required for this target.
    pub required: bool,
    /// Capability that makes an optional probe applicable, where one exists.
    pub capability: Option<Capability>,
}

/// A validated target and all twenty-three probe slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeMatrix {
    /// Exact pair being qualified.
    pub target: QualificationTarget,
    /// One slot for every registry probe, in id order.
    pub slots: Vec<ProbeSlot>,
    /// Accepted credential variable used by the readiness check.
    pub credential_variable: &'static str,
}

/// The interface a protected live runner must implement for one probe.
pub trait ProbeExecutor {
    /// Executes one required probe and returns its observed result.
    ///
    /// Implementations must perform real bounded provider I/O and return a
    /// `Pass` only after observing the declared behavior. They must never
    /// synthesize a pass from fixtures or model-list metadata.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError`] when the provider call cannot produce a
    /// truthful result.
    fn run_probe(
        &mut self,
        target: &QualificationTarget,
        probe: ProbeId,
    ) -> Result<ProbeRun, ReadinessError>;
}

/// Placeholder used until provider-specific live HTTP executors land.
#[derive(Debug, Default, Clone, Copy)]
pub struct PendingProbeExecutor;

impl ProbeExecutor for PendingProbeExecutor {
    fn run_probe(
        &mut self,
        _target: &QualificationTarget,
        probe: ProbeId,
    ) -> Result<ProbeRun, ReadinessError> {
        Err(ReadinessError::ExecutorUnavailable { probe })
    }
}

impl ProbeMatrix {
    /// Executes required slots and emits explicit `NotApplicable` rows for
    /// capability-gated slots. No slot is omitted, and no optional row can
    /// hide a missing unconditional probe.
    ///
    /// # Errors
    ///
    /// Propagates the runner's readiness error, or rejects a malformed runner
    /// result that does not identify the requested probe with a real pass or
    /// fail outcome.
    pub fn execute(
        &self,
        executor: &mut dyn ProbeExecutor,
    ) -> Result<Vec<ProbeRun>, ReadinessError> {
        let mut runs = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            if slot.required {
                let run = executor.run_probe(&self.target, slot.probe)?;
                if run.probe != slot.probe
                    || !matches!(&run.outcome, ProbeOutcome::Pass | ProbeOutcome::Fail { .. })
                {
                    return Err(ReadinessError::ExecutorUnavailable { probe: slot.probe });
                }
                runs.push(run);
            } else {
                let Some(capability) = slot.capability else {
                    return Err(ReadinessError::ExecutorUnavailable { probe: slot.probe });
                };
                runs.push(ProbeRun {
                    probe: slot.probe,
                    outcome: ProbeOutcome::NotApplicable { capability },
                    observed: Vec::new(),
                    duration_ms: 0,
                });
            }
        }
        Ok(runs)
    }
}

fn optional_capability(probe: ProbeId) -> Option<Capability> {
    probe.required_by().or(match probe {
        ProbeId::P07 => Some(Capability::ToolChoiceRequired),
        ProbeId::P12 => Some(Capability::PromptCacheExplicit),
        _ => None,
    })
}

fn matrix_slots(capabilities: CapabilitySet) -> Vec<ProbeSlot> {
    ProbeId::ALL
        .into_iter()
        .map(|probe| {
            let capability = optional_capability(probe);
            ProbeSlot {
                probe,
                required: probe.is_required_for(capabilities),
                capability,
            }
        })
        .collect()
}

/// Builds a matrix after a caller has proved that one accepted credential name
/// is present. This pure form is used by tests and by the protected runner's
/// credential-custody bridge; it never accepts or stores a secret value.
///
/// # Errors
///
/// Returns [`ReadinessError::MissingCredential`] when no accepted variable is
/// reported present.
pub fn prepare_with_credential_presence(
    target: QualificationTarget,
    credential_present: impl Fn(&str) -> bool,
) -> Result<ProbeMatrix, ReadinessError> {
    let profile = profile(target.provider);
    let credential_variable = profile
        .credential_variables
        .iter()
        .copied()
        .find(|name| credential_present(name))
        .ok_or(ReadinessError::MissingCredential {
            provider: target.provider,
            variables: profile.credential_variables,
        })?;
    Ok(ProbeMatrix {
        slots: matrix_slots(target.capabilities),
        target,
        credential_variable,
    })
}

/// Builds a matrix from the live harness environment.
///
/// The provider key is read through [`ProviderKeys::require`], which fails
/// loudly for absent/empty/non-Unicode values. The plaintext is not retained
/// in the matrix and is never printed; a real executor obtains its credential
/// through the protected custody path before dispatch.
///
/// # Errors
///
/// Returns [`ReadinessError::MissingCredential`] only if the environment reader
/// is extended to report a nonstandard failure after the required key check.
/// Missing, empty and non-Unicode values fail through [`ProviderKeys::require`]
/// before this function can return.
pub fn prepare_from_live_env(target: QualificationTarget) -> Result<ProbeMatrix, ReadinessError> {
    let _key = ProviderKeys::require(target.provider);
    prepare_with_credential_presence(target, |_| true)
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::document::{Capability, CapabilitySet};
    use aex_model_catalog::receipt::{ProbeId, ProbeOutcome};
    use aex_wire::provider::ProviderId;

    use super::{
        MOONSHOT_CAPABILITIES, PendingProbeExecutor, ProbeExecutor, QualificationTarget,
        ReadinessError, TEXT_STREAM, prepare_with_credential_presence, profile,
    };

    #[test]
    fn every_provider_has_explicit_credential_and_profile_metadata() {
        for provider in ProviderId::ALL {
            let item = profile(*provider);
            assert_eq!(item.provider, *provider);
            assert!(!item.credential_variables.is_empty());
            assert!(item.capability_ceiling.has(Capability::TextIn));
            assert!(item.capability_ceiling.has(Capability::TextOut));
            assert!(item.capability_ceiling.has(Capability::Streaming));
            assert_eq!(item.credential_variables, crate::key_variables(*provider));
        }
    }

    #[test]
    fn every_provider_requires_an_exact_model_input() {
        for provider in ProviderId::ALL {
            assert!(profile(*provider).requires_explicit_model);
        }
    }

    #[test]
    fn target_accepts_an_explicit_model_but_rejects_gateway_overclaims() {
        assert!(
            QualificationTarget::new(ProviderId::Openai, "provider-model", TEXT_STREAM).is_ok()
        );
        assert!(matches!(
            QualificationTarget::new(
                ProviderId::Openrouter,
                "gateway-model",
                TEXT_STREAM.with(Capability::Tools)
            ),
            Err(ReadinessError::UnsupportedCapability {
                capability: Capability::Tools,
                ..
            })
        ));
    }

    #[test]
    fn target_requires_text_input_output_and_streaming_capabilities() {
        assert!(matches!(
            QualificationTarget::new(ProviderId::Openai, "provider-model", CapabilitySet::EMPTY),
            Err(ReadinessError::MissingCapability {
                capability: Capability::TextIn,
                ..
            })
        ));
    }

    #[test]
    fn unknown_capability_bits_fail_without_being_relabelled() {
        assert!(matches!(
            QualificationTarget::new(ProviderId::Openai, "provider-model", CapabilitySet(u32::MAX)),
            Err(ReadinessError::UnknownCapabilityBits { bits, .. }) if bits != 0
        ));
    }

    #[test]
    fn missing_credential_fails_before_a_matrix_exists() {
        let target = QualificationTarget::new(ProviderId::Anthropic, "provider-model", TEXT_STREAM)
            .expect("candidate");
        let error = prepare_with_credential_presence(target, |_| false)
            .expect_err("missing customer key must fail closed");
        assert!(matches!(error, ReadinessError::MissingCredential { .. }));
    }

    #[test]
    fn only_documented_aliases_can_satisfy_presence() {
        let target = QualificationTarget::new(ProviderId::Anthropic, "provider-model", TEXT_STREAM)
            .expect("candidate");
        let matrix = prepare_with_credential_presence(target, |name| name == "ANTHROPIC_API_KEY")
            .expect("documented alias");
        assert_eq!(matrix.credential_variable, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn every_probe_gets_a_slot_and_pending_executor_cannot_emit_a_receipt() {
        let target = QualificationTarget::new(ProviderId::Openai, "provider-model", TEXT_STREAM)
            .expect("candidate");
        let matrix = prepare_with_credential_presence(target, |_| true).expect("key");
        assert_eq!(matrix.slots.len(), ProbeId::ALL.len());
        assert_eq!(
            matrix
                .slots
                .iter()
                .map(|slot| slot.probe)
                .collect::<Vec<_>>(),
            ProbeId::ALL.to_vec()
        );
        let error = matrix
            .execute(&mut PendingProbeExecutor)
            .expect_err("required P-01 has no implementation");
        assert_eq!(
            error,
            ReadinessError::ExecutorUnavailable {
                probe: ProbeId::P01
            }
        );
    }

    struct PassFixture;
    impl ProbeExecutor for PassFixture {
        fn run_probe(
            &mut self,
            _target: &QualificationTarget,
            probe: ProbeId,
        ) -> Result<crate::ProbeRun, ReadinessError> {
            Ok(crate::ProbeRun {
                probe,
                outcome: ProbeOutcome::Pass,
                observed: Vec::new(),
                duration_ms: 1,
            })
        }
    }

    #[test]
    fn optional_probes_are_explicit_not_applicable_rows() {
        let target = QualificationTarget::new(ProviderId::Openai, "provider-model", TEXT_STREAM)
            .expect("candidate");
        let matrix = prepare_with_credential_presence(target, |_| true).expect("key");
        let runs = matrix.execute(&mut PassFixture).expect("fixture runner");
        assert_eq!(runs.len(), ProbeId::ALL.len());
        let p03 = runs.iter().find(|run| run.probe == ProbeId::P03).unwrap();
        assert!(matches!(p03.outcome, ProbeOutcome::NotApplicable { .. }));
        let p01 = runs.iter().find(|run| run.probe == ProbeId::P01).unwrap();
        assert!(matches!(p01.outcome, ProbeOutcome::Pass));
    }

    #[test]
    fn profile_capability_ceiling_keeps_moonshot_reasoning_replay_unclaimed() {
        assert!(!MOONSHOT_CAPABILITIES.has(Capability::ReasoningReplay));
    }
}
