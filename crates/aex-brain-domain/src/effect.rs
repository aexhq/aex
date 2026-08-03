//! The split-phase durable effect state machine and its recovery matrix.
//!
//! `dispatch_started` is a separate durable write because a crash between `Prepared` and
//! the socket write must be distinguishable from a crash after it. Without that write both
//! look identical and every prepared effect becomes ambiguous, so every interrupted run
//! would have to be reported as possibly-charged.
//!
//! The whole point of this module is that **a possibly-received provider request never
//! becomes a silent retry or a second generation**.

use aex_wire::ids::GenerationId;
use serde::{Deserialize, Serialize};

use crate::ids::{ContentHash, DetachedOperationId, EffectId, ProviderRequestId, Timestamp};
use crate::journal::ExecutorRoute;

/// A detached tool operation plus the exact executor that accepted it.
///
/// Upstream operation ids are scoped to an executor. Persisting only the raw id would make
/// two executors that returned the same bytes collide and would force a restarted mux to
/// rediscover routing state it is not allowed to own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachedOperationRef {
    /// Executor-scoped operation identity.
    pub id: DetachedOperationId,
    /// The exact coarse Brain executor route that accepted the operation.
    pub executor: ExecutorRoute,
}

/// What kind of external work an effect performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// A provider model call.
    ModelCall,
    /// A Brain-side or managed tool invocation.
    ToolCall,
    /// A Hands `MicroVM` operation.
    HandsOperation,
    /// A durable timer or scheduled wait.
    DurableWait,
    /// A subagent fanout page.
    ChildSpawn,
}

impl EffectKind {
    /// The tag that goes into the effect identity derivation.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::ModelCall => 1,
            Self::ToolCall => 2,
            Self::HandsOperation => 3,
            Self::DurableWait => 4,
            Self::ChildSpawn => 5,
        }
    }
}

/// The recovery contract an effect declares before it is dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    /// Deterministic: recomputing under the same input hash is free of side effects.
    Pure,
    /// Registered with an idempotency key or a durable operation the catalog proved.
    IdempotentManaged,
    /// Dispatched once; the connection is released and the result queried later.
    DurableDetached,
    /// Arbitrary external mutation with no idempotency and no result lookup.
    ///
    /// Every launch model call is here: no `BYOK` provider exposes a proven
    /// generation-resume or result-lookup operation.
    NonReplayable,
}

/// What the adapter can prove about whether a request left the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchProof {
    /// The adapter proved no byte reached the upstream. The **only** value permitting an
    /// automatic retry.
    NotSent,
    /// Bytes may have reached the upstream.
    PossiblySent,
    /// The upstream began responding.
    ResponseStarted,
}

/// How far a failed dispatch got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStage {
    /// Failed before any socket write: request construction, permit, screening, auth.
    PreDispatch,
    /// The request was written.
    Dispatched,
    /// The response body had begun.
    Streaming,
    /// The upstream returned a definitive terminal condition.
    Terminal,
}

/// The durable evidence recorded about one dispatch attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchEvidence {
    /// How far the attempt got.
    pub stage: DispatchStage,
    /// What the adapter can prove.
    pub proof: DispatchProof,
    /// The attempt number, from one.
    pub attempt: u16,
    /// The upstream request id, when one was observed.
    pub provider_request_id: Option<ProviderRequestId>,
    /// A non-tool durable operation id, when an upstream accepted one.
    pub external_operation: Option<DetachedOperationId>,
    /// A detached tool operation bound to the executor that accepted it.
    pub detached_tool: Option<DetachedOperationRef>,
    /// A checksummed durable response receipt, when one committed.
    pub receipt: Option<ContentHash>,
    /// Redacted diagnostic text. Never carries a credential.
    pub detail: Option<String>,
}

impl DispatchEvidence {
    /// Evidence that nothing left the process.
    #[must_use]
    pub const fn not_sent(attempt: u16) -> Self {
        Self {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            attempt,
            provider_request_id: None,
            external_operation: None,
            detached_tool: None,
            receipt: None,
            detail: None,
        }
    }

    /// Evidence that the process lost ownership or died with a request in flight.
    #[must_use]
    pub const fn ambiguous(attempt: u16, stage: DispatchStage) -> Self {
        Self {
            stage,
            proof: DispatchProof::PossiblySent,
            attempt,
            provider_request_id: None,
            external_operation: None,
            detached_tool: None,
            receipt: None,
            detail: None,
        }
    }
}

/// Where a durable effect stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EffectState {
    /// Intent committed; nothing has been sent.
    Prepared {
        /// Which attempt this is.
        attempt: u16,
    },
    /// The durable pre-send write committed. A byte may now leave.
    DispatchStarted {
        /// Which attempt this is.
        attempt: u16,
    },
    /// The first validated response byte arrived.
    ResponseStarted {
        /// Which attempt this is.
        attempt: u16,
        /// The upstream request id, when one was observed.
        provider_request_id: Option<ProviderRequestId>,
    },
    /// The effect settled with a usable outcome.
    Complete {
        /// A checksummed receipt of the outcome.
        receipt: ContentHash,
    },
    /// The upstream returned a definitive terminal error.
    KnownFailure {
        /// How far the attempt got.
        stage: DispatchStage,
        /// What the adapter proved.
        proof: DispatchProof,
    },
    /// Nothing can be proved about the outcome. This is honest, not a failure to try.
    OutcomeUnknown {
        /// Everything recorded about the attempt.
        evidence: DispatchEvidence,
    },
}

impl EffectState {
    /// Whether this state admits no further transition.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        matches!(
            self,
            Self::Complete { .. } | Self::KnownFailure { .. } | Self::OutcomeUnknown { .. }
        )
    }

    /// The attempt this state belongs to, for the unsettled states.
    #[must_use]
    pub const fn attempt(&self) -> Option<u16> {
        match self {
            Self::Prepared { attempt }
            | Self::DispatchStarted { attempt }
            | Self::ResponseStarted { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }
}

/// How an effect settled, as `EffectSettled` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SettledOutcome {
    /// A usable result was produced.
    Complete {
        /// A checksummed receipt of the outcome.
        receipt: ContentHash,
    },
    /// The upstream said no, definitively.
    KnownFailure {
        /// How far the attempt got.
        stage: DispatchStage,
        /// What the adapter proved.
        proof: DispatchProof,
    },
    /// Nothing can be proved.
    OutcomeUnknown {
        /// Everything recorded about the attempt.
        evidence: DispatchEvidence,
    },
}

/// The durable effect record, as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableEffect {
    /// Deterministic identity.
    pub id: EffectId,
    /// What kind of work it performs.
    pub kind: EffectKind,
    /// Exact runtime generation for Hands work; absent for every other effect kind.
    pub generation: Option<GenerationId>,
    /// Its recovery contract.
    pub class: EffectClass,
    /// `blake3` over the canonical request. A second request under the same id is a fork.
    pub request_hash: ContentHash,
    /// Where it stands.
    pub state: EffectState,
    /// When the current attempt must have settled by.
    pub deadline: Timestamp,
    /// Everything recorded about the current attempt.
    pub evidence: Option<DispatchEvidence>,
}

/// What the owner should do with an effect it found open after a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RecoveryDecision {
    /// Retry under a new fence with the same effect id at `attempt + 1`.
    RetrySameEffect {
        /// The attempt number the retry runs as.
        attempt: u16,
    },
    /// Ask the upstream about the durable operation it accepted. Never create a second one.
    QueryDurableOperation {
        /// The operation the upstream accepted.
        id: DetachedOperationId,
    },
    /// Ask the exact tool executor about the detached operation it accepted.
    QueryDetachedTool {
        /// Executor-bound operation identity. Never broadcast or rediscover this route.
        operation: DetachedOperationRef,
    },
    /// Rebuild the outcome from the committed checksummed receipt.
    ReconstructFromReceipt {
        /// The receipt digest.
        receipt: ContentHash,
    },
    /// Settle `OutcomeUnknown` and terminalize the run `interrupted`.
    Interrupt {
        /// The evidence the settlement records.
        evidence: DispatchEvidence,
    },
}

/// Decides what to do with `effect`, given the catalog's durable-operation support.
///
/// The matrix is deliberately narrow. `RetrySameEffect` is reachable from
/// `DispatchStarted` only for [`EffectClass::Pure`], so a possibly-received provider
/// request can never become a second generation.
#[must_use]
pub fn recover(
    effect: &DurableEffect,
    support: crate::wire_pending::DurableOperationSupport,
) -> RecoveryDecision {
    use crate::wire_pending::DurableOperationSupport as Support;

    let evidence = effect.evidence.clone().unwrap_or_else(|| {
        DispatchEvidence::ambiguous(
            effect.state.attempt().unwrap_or(1),
            match effect.state {
                EffectState::Prepared { .. } => DispatchStage::PreDispatch,
                EffectState::ResponseStarted { .. } => DispatchStage::Streaming,
                _ => DispatchStage::Dispatched,
            },
        )
    });

    match &effect.state {
        // Intent committed, nothing sent. The one unambiguous case.
        EffectState::Prepared { attempt } => RecoveryDecision::RetrySameEffect {
            attempt: attempt.saturating_add(1),
        },
        // Already settled: recovery has nothing to decide.
        EffectState::Complete { receipt } => {
            RecoveryDecision::ReconstructFromReceipt { receipt: *receipt }
        }
        EffectState::KnownFailure { stage, proof } => RecoveryDecision::Interrupt {
            evidence: DispatchEvidence {
                stage: *stage,
                proof: *proof,
                ..evidence
            },
        },
        EffectState::OutcomeUnknown { evidence } => RecoveryDecision::Interrupt {
            evidence: evidence.clone(),
        },
        EffectState::DispatchStarted { attempt } | EffectState::ResponseStarted { attempt, .. } => {
            // A committed checksummed response receipt outranks everything else: the
            // outcome is already known and needs no upstream contact at all.
            if let Some(receipt) = evidence.receipt {
                return RecoveryDecision::ReconstructFromReceipt { receipt };
            }
            if evidence.external_operation.is_some() && evidence.detached_tool.is_some() {
                return RecoveryDecision::Interrupt { evidence };
            }
            match effect.class {
                EffectClass::Pure => RecoveryDecision::RetrySameEffect {
                    attempt: attempt.saturating_add(1),
                },
                EffectClass::IdempotentManaged => {
                    if !matches!(
                        support,
                        Support::ResultLookup { .. } | Support::ResumableStream { .. }
                    ) {
                        RecoveryDecision::Interrupt { evidence }
                    } else if effect.kind == EffectKind::ToolCall {
                        match evidence.detached_tool.clone() {
                            Some(operation) => RecoveryDecision::QueryDetachedTool { operation },
                            None => RecoveryDecision::Interrupt { evidence },
                        }
                    } else {
                        match evidence.external_operation.clone() {
                            Some(id) => RecoveryDecision::QueryDurableOperation { id },
                            None => RecoveryDecision::Interrupt { evidence },
                        }
                    }
                }
                EffectClass::DurableDetached => {
                    if effect.kind == EffectKind::ToolCall {
                        match evidence.detached_tool.clone() {
                            Some(operation) => RecoveryDecision::QueryDetachedTool { operation },
                            None => RecoveryDecision::Interrupt { evidence },
                        }
                    } else {
                        match evidence.external_operation.clone() {
                            Some(id) => RecoveryDecision::QueryDurableOperation { id },
                            None => RecoveryDecision::Interrupt { evidence },
                        }
                    }
                }
                EffectClass::NonReplayable => RecoveryDecision::Interrupt { evidence },
            }
        }
    }
}

/// Why a transition on an effect was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffectError {
    /// The effect was not in the state the transition requires.
    #[error("effect {id} is {found:?}, not {expected}")]
    StateMismatch {
        /// The effect.
        id: EffectId,
        /// What it actually is.
        found: Box<EffectState>,
        /// What the transition required.
        expected: &'static str,
    },
    /// The same effect id arrived carrying a different request. The agent quarantines.
    #[error("effect {id} already exists under a different request hash")]
    RequestConflict {
        /// The effect.
        id: EffectId,
    },
}

impl DurableEffect {
    /// Moves `Prepared -> DispatchStarted`.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::StateMismatch`] unless the effect is `Prepared`.
    pub fn mark_dispatch_started(&mut self) -> Result<(), EffectError> {
        match self.state {
            EffectState::Prepared { attempt } => {
                self.state = EffectState::DispatchStarted { attempt };
                self.evidence = Some(DispatchEvidence::ambiguous(
                    attempt,
                    DispatchStage::Dispatched,
                ));
                Ok(())
            }
            ref other => Err(EffectError::StateMismatch {
                id: self.id,
                found: Box::new(other.clone()),
                expected: "prepared",
            }),
        }
    }

    /// Moves `DispatchStarted -> ResponseStarted`.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::StateMismatch`] unless the effect is `DispatchStarted`.
    pub fn mark_response_started(&mut self, evidence: DispatchEvidence) -> Result<(), EffectError> {
        match self.state {
            EffectState::DispatchStarted { attempt } => {
                self.state = EffectState::ResponseStarted {
                    attempt,
                    provider_request_id: evidence.provider_request_id.clone(),
                };
                self.evidence = Some(evidence);
                Ok(())
            }
            ref other => Err(EffectError::StateMismatch {
                id: self.id,
                found: Box::new(other.clone()),
                expected: "dispatch_started",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
        EffectClass, EffectError, EffectKind, EffectState, RecoveryDecision, recover,
    };
    use crate::ids::{ContentHash, DetachedOperationId, EffectId, Timestamp};
    use crate::journal::ExecutorRoute;
    use crate::wire_pending::DurableOperationSupport;

    const PROVEN: DurableOperationSupport =
        DurableOperationSupport::ResultLookup { ttl_ms: 60_000 };

    fn effect(state: EffectState, class: EffectClass) -> DurableEffect {
        DurableEffect {
            id: EffectId([7; 16]),
            kind: EffectKind::ModelCall,
            generation: None,
            class,
            request_hash: ContentHash::of(b"request"),
            state,
            deadline: Timestamp::from_millis(1_000),
            evidence: None,
        }
    }

    #[test]
    fn a_prepared_effect_is_the_only_unambiguous_retry() {
        let decision = recover(
            &effect(
                EffectState::Prepared { attempt: 1 },
                EffectClass::NonReplayable,
            ),
            DurableOperationSupport::None,
        );
        assert_eq!(decision, RecoveryDecision::RetrySameEffect { attempt: 2 });
    }

    #[test]
    fn a_dispatched_non_replayable_effect_interrupts_and_never_retries() {
        for state in [
            EffectState::DispatchStarted { attempt: 1 },
            EffectState::ResponseStarted {
                attempt: 1,
                provider_request_id: None,
            },
        ] {
            let decision = recover(&effect(state, EffectClass::NonReplayable), PROVEN);
            assert!(
                matches!(decision, RecoveryDecision::Interrupt { .. }),
                "{decision:?}"
            );
        }
    }

    #[test]
    fn an_idempotent_managed_effect_needs_both_a_proven_capability_and_an_operation() {
        let mut with_operation = effect(
            EffectState::DispatchStarted { attempt: 1 },
            EffectClass::IdempotentManaged,
        );
        with_operation.evidence = Some(DispatchEvidence {
            external_operation: Some(DetachedOperationId("op-1".to_owned())),
            ..DispatchEvidence::ambiguous(1, DispatchStage::Dispatched)
        });
        assert!(matches!(
            recover(&with_operation, PROVEN),
            RecoveryDecision::QueryDurableOperation { .. }
        ));
        assert!(
            matches!(
                recover(&with_operation, DurableOperationSupport::None),
                RecoveryDecision::Interrupt { .. }
            ),
            "an unproven capability may not be queried"
        );
        let without_operation = effect(
            EffectState::DispatchStarted { attempt: 1 },
            EffectClass::IdempotentManaged,
        );
        assert!(matches!(
            recover(&without_operation, PROVEN),
            RecoveryDecision::Interrupt { .. }
        ));
    }

    #[test]
    fn a_committed_receipt_outranks_the_class() {
        let mut settled = effect(
            EffectState::ResponseStarted {
                attempt: 1,
                provider_request_id: None,
            },
            EffectClass::NonReplayable,
        );
        let receipt = ContentHash::of(b"response");
        settled.evidence = Some(DispatchEvidence {
            receipt: Some(receipt),
            ..DispatchEvidence::ambiguous(1, DispatchStage::Streaming)
        });
        assert_eq!(
            recover(&settled, DurableOperationSupport::None),
            RecoveryDecision::ReconstructFromReceipt { receipt }
        );
    }

    #[test]
    fn a_detached_operation_ref_has_one_closed_persisted_shape() {
        let reference = DetachedOperationRef {
            id: DetachedOperationId("same-id".to_owned()),
            executor: ExecutorRoute::Mcp,
        };
        let encoded = serde_json::to_vec(&reference).expect("the reference serializes");
        assert_eq!(
            serde_json::from_slice::<DetachedOperationRef>(&encoded)
                .expect("the reference round trips"),
            reference
        );
        assert!(
            serde_json::from_str::<DetachedOperationRef>(
                r#"{"id":"same-id","executor":"mcp","broadcast":true}"#,
            )
            .is_err(),
            "unknown routing fields must fail closed"
        );
    }

    #[test]
    fn a_pure_effect_recomputes_under_the_same_input_hash() {
        let decision = recover(
            &effect(
                EffectState::ResponseStarted {
                    attempt: 3,
                    provider_request_id: None,
                },
                EffectClass::Pure,
            ),
            DurableOperationSupport::None,
        );
        assert_eq!(decision, RecoveryDecision::RetrySameEffect { attempt: 4 });
    }

    #[test]
    fn dispatch_started_may_only_be_marked_once() {
        let mut prepared = effect(
            EffectState::Prepared { attempt: 1 },
            EffectClass::NonReplayable,
        );
        prepared
            .mark_dispatch_started()
            .expect("the first mark is admitted");
        let error = prepared
            .mark_dispatch_started()
            .expect_err("the second mark is refused");
        assert!(
            matches!(
                error,
                EffectError::StateMismatch {
                    expected: "prepared",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn response_started_requires_a_dispatch_started_effect() {
        let mut prepared = effect(
            EffectState::Prepared { attempt: 1 },
            EffectClass::NonReplayable,
        );
        let error = prepared
            .mark_response_started(DispatchEvidence {
                stage: DispatchStage::Streaming,
                proof: DispatchProof::ResponseStarted,
                ..DispatchEvidence::not_sent(1)
            })
            .expect_err("a response cannot start before a dispatch");
        assert!(
            matches!(
                error,
                EffectError::StateMismatch {
                    expected: "dispatch_started",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn every_effect_kind_has_a_distinct_identity_tag() {
        let mut tags: Vec<u8> = [
            EffectKind::ModelCall,
            EffectKind::ToolCall,
            EffectKind::HandsOperation,
            EffectKind::DurableWait,
            EffectKind::ChildSpawn,
        ]
        .iter()
        .map(|kind| kind.tag())
        .collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), 5);
    }
}
