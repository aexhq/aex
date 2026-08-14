//! Slice S-5.2 — the recovery truth table as an exhaustive property over
//! `(state, class, capability)`.
//!
//! The one rule the whole table exists to enforce: **a possibly-received provider request
//! never becomes a silent retry or a second generation.** Every other row is subordinate to
//! it.

use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind, EffectState, RecoveryDecision, recover,
};
use aex_brain_domain::ids::{
    ContentHash, DetachedOperationId, EffectId, ProviderRequestId, Timestamp,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::wire_pending::DurableOperationSupport;
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

const PROVEN: DurableOperationSupport = DurableOperationSupport::ResultLookup { ttl_ms: 60_000 };

const STATES: [fn() -> EffectState; 6] = [
    || EffectState::Prepared { attempt: 1 },
    || EffectState::DispatchStarted { attempt: 1 },
    || EffectState::ResponseStarted {
        attempt: 1,
        provider_request_id: Some(ProviderRequestId::truncating("req_1")),
    },
    || EffectState::Complete {
        receipt: ContentHash::of(b"receipt"),
    },
    || EffectState::KnownFailure {
        stage: DispatchStage::Terminal,
        proof: DispatchProof::PossiblySent,
    },
    || EffectState::OutcomeUnknown {
        evidence: DispatchEvidence::ambiguous(1, DispatchStage::Streaming),
    },
];

const CLASSES: [EffectClass; 4] = [
    EffectClass::Pure,
    EffectClass::IdempotentManaged,
    EffectClass::DurableDetached,
    EffectClass::NonReplayable,
];

const SUPPORT: [DurableOperationSupport; 2] = [DurableOperationSupport::None, PROVEN];

const KINDS: [EffectKind; 5] = [
    EffectKind::ModelCall,
    EffectKind::ToolCall,
    EffectKind::HandsOperation,
    EffectKind::DurableWait,
    EffectKind::ChildSpawn,
];

fn effect(
    state: EffectState,
    class: EffectClass,
    kind: EffectKind,
    evidence: Option<DispatchEvidence>,
) -> DurableEffect {
    DurableEffect {
        id: EffectId([7; 16]),
        kind,
        generation: matches!(kind, EffectKind::HandsOperation)
            .then(|| GenerationId::from_uuid7(Uuid7::compose(1, [7; 10]))),
        class,
        request_hash: ContentHash::of(b"request"),
        state,
        deadline: Timestamp(60_000),
        evidence,
    }
}

fn with_external_operation(attempt: u16, stage: DispatchStage) -> DispatchEvidence {
    DispatchEvidence {
        external_operation: Some(DetachedOperationId("op_1".to_owned())),
        ..DispatchEvidence::ambiguous(attempt, stage)
    }
}

fn with_detached_tool(attempt: u16, stage: DispatchStage) -> DispatchEvidence {
    DispatchEvidence {
        detached_tool: Some(DetachedOperationRef {
            id: DetachedOperationId("op_1".to_owned()),
            executor: ExecutorRoute::ToolMux,
        }),
        ..DispatchEvidence::ambiguous(attempt, stage)
    }
}

fn with_receipt(attempt: u16, stage: DispatchStage) -> DispatchEvidence {
    DispatchEvidence {
        receipt: Some(ContentHash::of(b"committed receipt")),
        ..DispatchEvidence::ambiguous(attempt, stage)
    }
}

/// The whole cross product is defined and stable.
#[test]
fn the_matrix_is_total_and_deterministic() {
    for build in STATES {
        for class in CLASSES {
            for support in SUPPORT {
                for kind in KINDS {
                    for evidence in [
                        None,
                        Some(DispatchEvidence::ambiguous(1, DispatchStage::Dispatched)),
                        Some(with_external_operation(1, DispatchStage::Dispatched)),
                        Some(with_detached_tool(1, DispatchStage::Dispatched)),
                        Some(with_receipt(1, DispatchStage::Streaming)),
                    ] {
                        let subject = effect(build(), class, kind, evidence);
                        let first = recover(&subject, support);
                        assert_eq!(
                            first,
                            recover(&subject, support),
                            "{:?}/{class:?}/{support:?}/{kind:?}",
                            subject.state
                        );
                    }
                }
            }
        }
    }
}

/// `Prepared` is the one unambiguous case: nothing left, so retry the same identity.
#[test]
fn prepared_always_retries_the_same_effect() {
    for class in CLASSES {
        for support in SUPPORT {
            let subject = effect(
                EffectState::Prepared { attempt: 3 },
                class,
                EffectKind::ModelCall,
                None,
            );
            assert_eq!(
                recover(&subject, support),
                RecoveryDecision::RetrySameEffect { attempt: 4 },
                "{class:?}/{support:?}"
            );
        }
    }
}

/// A dispatched effect retries only when recomputing it is free of side effects.
///
/// This is the guard that stops a possibly-received provider request from becoming a second
/// generation. Everything that is not `Pure` must reach the upstream by identity or settle
/// honestly unknown.
#[test]
fn only_a_pure_effect_retries_after_dispatch() {
    for state in [
        EffectState::DispatchStarted { attempt: 2 },
        EffectState::ResponseStarted {
            attempt: 2,
            provider_request_id: None,
        },
    ] {
        for class in CLASSES {
            for support in SUPPORT {
                let subject = effect(
                    state.clone(),
                    class,
                    EffectKind::ModelCall,
                    Some(DispatchEvidence::ambiguous(2, DispatchStage::Dispatched)),
                );
                let decision = recover(&subject, support);
                let retried = matches!(decision, RecoveryDecision::RetrySameEffect { .. });
                assert_eq!(
                    retried,
                    class == EffectClass::Pure,
                    "{state:?}/{class:?}/{support:?} produced {decision:?}"
                );
            }
        }
    }
}

/// Every launch model call is `NonReplayable`, so an ambiguous dispatch interrupts.
#[test]
fn an_ambiguous_model_call_interrupts_rather_than_regenerating() {
    for support in SUPPORT {
        let subject = effect(
            EffectState::DispatchStarted { attempt: 1 },
            EffectClass::NonReplayable,
            EffectKind::ModelCall,
            Some(DispatchEvidence::ambiguous(1, DispatchStage::Dispatched)),
        );
        assert!(
            matches!(
                recover(&subject, support),
                RecoveryDecision::Interrupt { .. }
            ),
            "{support:?}"
        );
    }
}

/// A managed effect queries its durable operation only when the catalog proved one exists
/// **and** the adapter actually recorded an operation id. Either missing means interrupt;
/// guessing would fabricate a recovery that has no upstream to query.
#[test]
fn a_managed_effect_queries_only_with_proof_and_an_operation_id() {
    let cases = [
        (PROVEN, true, true),
        (PROVEN, false, false),
        (DurableOperationSupport::None, true, false),
        (DurableOperationSupport::None, false, false),
    ];
    for (support, has_operation, expect_query) in cases {
        let evidence = if has_operation {
            with_detached_tool(1, DispatchStage::Dispatched)
        } else {
            DispatchEvidence::ambiguous(1, DispatchStage::Dispatched)
        };
        let subject = effect(
            EffectState::DispatchStarted { attempt: 1 },
            EffectClass::IdempotentManaged,
            EffectKind::ToolCall,
            Some(evidence),
        );
        let decision = recover(&subject, support);
        assert_eq!(
            matches!(decision, RecoveryDecision::QueryDetachedTool { .. }),
            expect_query,
            "{support:?}/operation={has_operation} produced {decision:?}"
        );
    }
}

/// A detached effect owns its operation id, so it queries without needing catalog proof —
/// the guest or server already accepted it.
#[test]
fn a_detached_effect_queries_its_own_accepted_operation() {
    for support in SUPPORT {
        let subject = effect(
            EffectState::DispatchStarted { attempt: 1 },
            EffectClass::DurableDetached,
            EffectKind::HandsOperation,
            Some(with_external_operation(1, DispatchStage::Dispatched)),
        );
        assert_eq!(
            recover(&subject, support),
            RecoveryDecision::QueryDurableOperation {
                id: DetachedOperationId("op_1".to_owned())
            },
            "{support:?}"
        );
    }
}

/// A tool operation is bound to its exact executor and conflicting bindings refuse.
#[test]
fn a_detached_tool_route_is_durable_and_operation_bindings_are_exclusive() {
    let detached = effect(
        EffectState::DispatchStarted { attempt: 1 },
        EffectClass::DurableDetached,
        EffectKind::ToolCall,
        Some(with_detached_tool(1, DispatchStage::Dispatched)),
    );
    assert_eq!(
        recover(&detached, DurableOperationSupport::None),
        RecoveryDecision::QueryDetachedTool {
            operation: DetachedOperationRef {
                id: DetachedOperationId("op_1".to_owned()),
                executor: ExecutorRoute::ToolMux,
            }
        }
    );

    let mut conflicting = with_detached_tool(1, DispatchStage::Dispatched);
    conflicting.external_operation = Some(DetachedOperationId("provider-op".to_owned()));
    let conflicting = effect(
        EffectState::DispatchStarted { attempt: 1 },
        EffectClass::DurableDetached,
        EffectKind::ToolCall,
        Some(conflicting),
    );
    assert!(matches!(
        recover(&conflicting, DurableOperationSupport::None),
        RecoveryDecision::Interrupt { .. }
    ));
}

/// A committed checksummed receipt outranks everything: the outcome is already known and
/// needs no upstream contact at all.
#[test]
fn a_committed_receipt_outranks_every_other_row() {
    for class in CLASSES {
        for support in SUPPORT {
            for state in [
                EffectState::DispatchStarted { attempt: 1 },
                EffectState::ResponseStarted {
                    attempt: 1,
                    provider_request_id: None,
                },
            ] {
                let subject = effect(
                    state,
                    class,
                    EffectKind::ModelCall,
                    Some(with_receipt(1, DispatchStage::Streaming)),
                );
                assert!(
                    matches!(
                        recover(&subject, support),
                        RecoveryDecision::ReconstructFromReceipt { .. }
                    ),
                    "{class:?}/{support:?}"
                );
            }
        }
    }
}

/// A settled effect has nothing to decide.
#[test]
fn a_settled_effect_reports_its_settlement() {
    let complete = effect(
        EffectState::Complete {
            receipt: ContentHash::of(b"done"),
        },
        EffectClass::NonReplayable,
        EffectKind::ModelCall,
        None,
    );
    assert_eq!(
        recover(&complete, DurableOperationSupport::None),
        RecoveryDecision::ReconstructFromReceipt {
            receipt: ContentHash::of(b"done")
        }
    );

    for state in [
        EffectState::KnownFailure {
            stage: DispatchStage::Terminal,
            proof: DispatchProof::PossiblySent,
        },
        EffectState::OutcomeUnknown {
            evidence: DispatchEvidence::ambiguous(1, DispatchStage::Streaming),
        },
    ] {
        let subject = effect(state, EffectClass::Pure, EffectKind::ToolCall, None);
        assert!(
            matches!(
                recover(&subject, DurableOperationSupport::None),
                RecoveryDecision::Interrupt { .. }
            ),
            "a settled effect must not be retried"
        );
    }
}

/// The split-phase transitions are one-way and refuse to skip a phase.
#[test]
fn the_split_phase_transitions_refuse_to_skip() {
    let mut subject = effect(
        EffectState::Prepared { attempt: 1 },
        EffectClass::NonReplayable,
        EffectKind::ModelCall,
        None,
    );
    subject
        .mark_response_started(DispatchEvidence::ambiguous(1, DispatchStage::Streaming))
        .expect_err("a response cannot start before a dispatch does");

    subject
        .mark_dispatch_started()
        .expect("prepared moves to dispatch started");
    assert!(matches!(
        subject.state,
        EffectState::DispatchStarted { attempt: 1 }
    ));
    subject
        .mark_dispatch_started()
        .expect_err("dispatch starts exactly once");

    subject
        .mark_response_started(DispatchEvidence {
            provider_request_id: Some(ProviderRequestId::truncating("req_9")),
            ..DispatchEvidence::ambiguous(1, DispatchStage::Streaming)
        })
        .expect("a validated byte moves it on");
    assert!(matches!(
        subject.state,
        EffectState::ResponseStarted { attempt: 1, .. }
    ));
}

/// `DispatchProof::NotSent` is the only value that permits an automatic retry.
#[test]
fn not_sent_is_the_only_proof_that_permits_a_retry() {
    for proof in [
        DispatchProof::NotSent,
        DispatchProof::PossiblySent,
        DispatchProof::ResponseStarted,
    ] {
        // `Prepared` means the durable pre-send write never happened, which is exactly what
        // `NotSent` asserts; every other proof belongs to a state past it.
        let state = if proof == DispatchProof::NotSent {
            EffectState::Prepared { attempt: 1 }
        } else {
            EffectState::DispatchStarted { attempt: 1 }
        };
        let subject = effect(
            state,
            EffectClass::NonReplayable,
            EffectKind::ModelCall,
            Some(DispatchEvidence {
                proof,
                ..DispatchEvidence::ambiguous(1, DispatchStage::Dispatched)
            }),
        );
        let decision = recover(&subject, DurableOperationSupport::None);
        assert_eq!(
            matches!(decision, RecoveryDecision::RetrySameEffect { .. }),
            proof == DispatchProof::NotSent,
            "{proof:?} produced {decision:?}"
        );
    }
}
