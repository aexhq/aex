//! Reading a durable effect row back as a [`DurableEffect`].
//!
//! The stored `state` is what a new owner classifies an interrupted attempt by, so it is
//! decoded strictly: an unknown spelling is a refusal, never `Prepared`. Treating an
//! unrecognised state as prepared is precisely how a possibly-sent provider request becomes
//! a second generation.

use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind, EffectState,
};
use aex_brain_domain::ids::{
    ContentHash, DetachedOperationId, EffectId, ProviderRequestId, Timestamp,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_session_dynamodb::attr::{CodecError, Item, Row};
use aex_wire::ids::GenerationId;

/// The `itemType` a durable effect row declares.
pub const AGENT_EFFECT: &str = aex_session_dynamodb::codec::AGENT_EFFECT;

/// Decodes one effect row.
///
/// # Errors
///
/// [`CodecError`] when the row is not an effect row, an attribute is absent or of the wrong
/// type, or a state, kind or class spelling is outside its closed set.
pub fn decode(item: &Item) -> Result<DurableEffect, CodecError> {
    let row = Row::bind(item, AGENT_EFFECT)?;
    let id = parse_id(row.string("effectId")?)?;
    let kind = parse_kind(row.string("kind")?)?;
    let generation = row.opt_id::<GenerationId>("generationId")?;
    match (kind, generation) {
        (EffectKind::HandsOperation, None) => {
            return Err(CodecError::Missing {
                item_type: AGENT_EFFECT,
                attribute: "generationId",
            });
        }
        (EffectKind::HandsOperation, Some(_)) | (_, None) => {}
        (_, Some(_)) => {
            return Err(malformed(
                "generationId",
                "only a HandsOperation may carry a runtime generation".to_owned(),
            ));
        }
    }
    let attempt = u16::try_from(row.opt_u64("attempt")?.unwrap_or(0)).unwrap_or(u16::MAX);
    let evidence = decode_evidence(&row, attempt, kind)?;
    Ok(DurableEffect {
        id,
        kind,
        generation,
        class: parse_class(row.opt_string("effectClass")?.unwrap_or("NonReplayable"))?,
        request_hash: parse_hash(row.string("requestHash")?)?,
        state: parse_state(row.string("state")?, attempt, evidence.clone())?,
        deadline: row
            .opt_timestamp("deadline")?
            .map_or(Timestamp::from_millis(0), |value| {
                Timestamp::from_millis(value.unix_millis())
            }),
        evidence,
    })
}

fn decode_evidence(
    row: &Row<'_>,
    attempt: u16,
    kind: EffectKind,
) -> Result<Option<DispatchEvidence>, CodecError> {
    let stage = row.opt_string("dispatchStage")?;
    let proof = row.opt_string("dispatchProof")?;
    let provider_request_id = row.opt_string("providerRequestId")?;
    let external_operation = row
        .opt_string("externalOperationId")?
        .map(|text| DetachedOperationId(text.to_owned()));
    let detached_id = row.opt_string("detachedOperationId")?;
    let detached_executor = row.opt_string("detachedExecutor")?;
    let receipt = row.opt_string("receiptHash")?;
    let Some(stage) = stage else {
        if proof.is_some()
            || provider_request_id.is_some()
            || external_operation.is_some()
            || detached_id.is_some()
            || detached_executor.is_some()
            || receipt.is_some()
        {
            return Err(malformed(
                "dispatchStage",
                "dispatch evidence attributes require a stage".to_owned(),
            ));
        }
        return Ok(None);
    };
    let detached_tool = match (detached_id, detached_executor) {
        (Some(id), Some(executor)) => Some(DetachedOperationRef {
            id: DetachedOperationId(id.to_owned()),
            executor: parse_executor(executor)?,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(malformed(
                "detachedExecutor",
                "a detached tool operation requires its exact executor".to_owned(),
            ));
        }
        (None, Some(_)) => {
            return Err(malformed(
                "detachedOperationId",
                "a detached executor requires its operation id".to_owned(),
            ));
        }
    };
    if external_operation.is_some() && detached_tool.is_some() {
        return Err(malformed(
            "externalOperationId",
            "external and detached-tool operation bindings are mutually exclusive".to_owned(),
        ));
    }
    if detached_tool.is_some() && kind != EffectKind::ToolCall {
        return Err(malformed(
            "detachedExecutor",
            "only a ToolCall may carry a detached tool operation".to_owned(),
        ));
    }
    Ok(Some(DispatchEvidence {
        attempt,
        stage: parse_stage(stage)?,
        proof: parse_proof(proof.unwrap_or("PossiblySent"))?,
        provider_request_id: provider_request_id
            .map(|text| {
                ProviderRequestId::new(text)
                    .map_err(|error| malformed("providerRequestId", error.to_string()))
            })
            .transpose()?,
        external_operation,
        detached_tool,
        receipt: receipt.map(parse_hash).transpose()?,
        detail: None,
    }))
}

fn parse_executor(text: &str) -> Result<ExecutorRoute, CodecError> {
    Ok(match text {
        "BrainInline" => ExecutorRoute::BrainInline,
        "ToolMux" => ExecutorRoute::ToolMux,
        other => {
            return Err(malformed(
                "detachedExecutor",
                format!("`{other}` is not an executor route"),
            ));
        }
    })
}

fn malformed(attribute: &'static str, reason: String) -> CodecError {
    CodecError::Malformed {
        item_type: AGENT_EFFECT,
        attribute,
        reason,
    }
}

fn parse_id(text: &str) -> Result<EffectId, CodecError> {
    if text.len() != 32 {
        return Err(malformed("effectId", format!("`{text}` is not 16 bytes")));
    }
    let mut bytes = [0_u8; 16];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let pair = text
            .get(index * 2..index * 2 + 2)
            .ok_or_else(|| malformed("effectId", "not hexadecimal".to_owned()))?;
        *slot = u8::from_str_radix(pair, 16)
            .map_err(|_| malformed("effectId", "not hexadecimal".to_owned()))?;
    }
    Ok(EffectId(bytes))
}

fn parse_hash(text: &str) -> Result<ContentHash, CodecError> {
    if text.len() != 64 {
        return Err(malformed(
            "requestHash",
            format!("`{text}` is not 32 bytes"),
        ));
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let pair = text
            .get(index * 2..index * 2 + 2)
            .ok_or_else(|| malformed("requestHash", "not hexadecimal".to_owned()))?;
        *slot = u8::from_str_radix(pair, 16)
            .map_err(|_| malformed("requestHash", "not hexadecimal".to_owned()))?;
    }
    Ok(ContentHash(bytes))
}

fn parse_kind(text: &str) -> Result<EffectKind, CodecError> {
    Ok(match text {
        "ModelCall" => EffectKind::ModelCall,
        "ToolCall" => EffectKind::ToolCall,
        "HandsOperation" => EffectKind::HandsOperation,
        "ChildSpawn" => EffectKind::ChildSpawn,
        other => {
            return Err(malformed(
                "kind",
                format!("`{other}` is not an effect kind"),
            ));
        }
    })
}

fn parse_class(text: &str) -> Result<EffectClass, CodecError> {
    Ok(match text {
        "Pure" => EffectClass::Pure,
        "IdempotentManaged" => EffectClass::IdempotentManaged,
        "DurableDetached" => EffectClass::DurableDetached,
        "NonReplayable" => EffectClass::NonReplayable,
        other => {
            return Err(malformed(
                "effectClass",
                format!("`{other}` is not an effect class"),
            ));
        }
    })
}

fn parse_stage(text: &str) -> Result<DispatchStage, CodecError> {
    Ok(match text {
        "PreDispatch" => DispatchStage::PreDispatch,
        "Dispatched" => DispatchStage::Dispatched,
        "Streaming" => DispatchStage::Streaming,
        "Terminal" => DispatchStage::Terminal,
        other => {
            return Err(malformed(
                "dispatchStage",
                format!("`{other}` is not a dispatch stage"),
            ));
        }
    })
}

fn parse_proof(text: &str) -> Result<DispatchProof, CodecError> {
    Ok(match text {
        "NotSent" => DispatchProof::NotSent,
        "PossiblySent" => DispatchProof::PossiblySent,
        "ResponseStarted" => DispatchProof::ResponseStarted,
        other => {
            return Err(malformed(
                "dispatchProof",
                format!("`{other}` is not a dispatch proof"),
            ));
        }
    })
}

fn parse_state(
    text: &str,
    attempt: u16,
    evidence: Option<DispatchEvidence>,
) -> Result<EffectState, CodecError> {
    Ok(match text {
        "prepared" => EffectState::Prepared { attempt },
        "dispatched" => EffectState::DispatchStarted { attempt },
        "responding" => EffectState::ResponseStarted {
            attempt,
            provider_request_id: evidence.and_then(|it| it.provider_request_id),
        },
        // A settled row is decoded as the ambiguous terminal rather than as a specific
        // outcome: the authoritative outcome lives in the journal, which is where it was
        // written atomically with the settlement.
        "settled" | "unknown" => EffectState::OutcomeUnknown {
            evidence: evidence
                .unwrap_or_else(|| DispatchEvidence::ambiguous(attempt, DispatchStage::Terminal)),
        },
        other => {
            return Err(malformed(
                "state",
                format!("`{other}` is not an effect state"),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{AGENT_EFFECT, decode};
    use aex_brain_domain::effect::{DetachedOperationRef, EffectClass, EffectKind, EffectState};
    use aex_brain_domain::ids::DetachedOperationId;
    use aex_brain_domain::journal::ExecutorRoute;
    use aex_session_dynamodb::attr::{ItemBuilder, n, s};
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [7; 10]))
    }

    fn row(state: &str) -> ItemBuilder {
        ItemBuilder::new(AGENT_EFFECT)
            .set("effectId", s("0".repeat(31) + "1"))
            .set("kind", s("ModelCall"))
            .set("effectClass", s("NonReplayable"))
            .set("requestHash", s("a".repeat(64)))
            .set("attempt", n(2))
            .set("state", s(state))
    }

    #[test]
    fn a_prepared_effect_decodes_as_retryable_state() {
        let effect = decode(&row("prepared").build()).expect("a well-formed row");
        assert_eq!(effect.state, EffectState::Prepared { attempt: 2 });
        assert_eq!(effect.kind, EffectKind::ModelCall);
        assert_eq!(effect.class, EffectClass::NonReplayable);
        assert!(!effect.state.is_settled());
    }

    #[test]
    fn a_dispatched_effect_carries_its_attempt() {
        let effect = decode(&row("dispatched").build()).expect("a well-formed row");
        assert_eq!(effect.state, EffectState::DispatchStarted { attempt: 2 });
    }

    /// The decisive one. If an unrecognised state decoded as `Prepared`, a new owner would
    /// retry a request that may already have reached the provider.
    #[test]
    fn an_unknown_state_is_refused_rather_than_treated_as_prepared() {
        let error = decode(&row("in_flight_probably").build())
            .expect_err("an unknown state has no safe reading");
        assert!(format!("{error}").contains("state"), "{error}");
    }

    #[test]
    fn a_settled_row_decodes_as_ambiguous_rather_than_as_a_specific_outcome() {
        let effect = decode(&row("settled").build()).expect("a well-formed row");
        assert!(
            matches!(effect.state, EffectState::OutcomeUnknown { .. }),
            "the authoritative outcome lives in the journal: {:?}",
            effect.state
        );
        assert!(effect.state.is_settled());
    }

    #[test]
    fn a_malformed_identity_is_refused() {
        let item = row("prepared").set("effectId", s("short")).build();
        assert!(decode(&item).is_err());
        let item = row("prepared").set("requestHash", s("zz")).build();
        assert!(decode(&item).is_err());
    }

    #[test]
    fn an_overlong_provider_request_id_is_refused_rather_than_truncated() {
        let item = row("responding")
            .set("dispatchStage", s("Streaming"))
            .set("dispatchProof", s("ResponseStarted"))
            .set("providerRequestId", s("r".repeat(81)))
            .build();
        let error = decode(&item).expect_err("stored authority is decoded strictly");
        assert!(format!("{error}").contains("providerRequestId"), "{error}");
    }

    #[test]
    fn a_detached_tool_binding_round_trips_with_its_executor() {
        let item = row("responding")
            .set("kind", s("ToolCall"))
            .set("dispatchStage", s("Streaming"))
            .set("dispatchProof", s("ResponseStarted"))
            .set("detachedOperationId", s("same-id"))
            .set("detachedExecutor", s("ToolMux"))
            .build();
        let effect = decode(&item).expect("closed detached operation binding");
        assert_eq!(
            effect.evidence.and_then(|value| value.detached_tool),
            Some(DetachedOperationRef {
                id: DetachedOperationId("same-id".to_owned()),
                executor: ExecutorRoute::ToolMux,
            })
        );
    }

    #[test]
    fn detached_tool_binding_refuses_half_pairs_unknown_routes_and_wrong_kinds() {
        for item in [
            row("responding")
                .set("kind", s("ToolCall"))
                .set("dispatchStage", s("Streaming"))
                .set("detachedOperationId", s("op"))
                .build(),
            row("responding")
                .set("kind", s("ToolCall"))
                .set("dispatchStage", s("Streaming"))
                .set("detachedExecutor", s("Mcp"))
                .build(),
            row("responding")
                .set("kind", s("ToolCall"))
                .set("dispatchStage", s("Streaming"))
                .set("detachedOperationId", s("op"))
                .set("detachedExecutor", s("Broadcast"))
                .build(),
            row("responding")
                .set("dispatchStage", s("Streaming"))
                .set("detachedOperationId", s("op"))
                .set("detachedExecutor", s("Mcp"))
                .build(),
        ] {
            assert!(decode(&item).is_err(), "{item:?}");
        }
    }

    #[test]
    fn external_and_detached_tool_bindings_are_mutually_exclusive() {
        let item = row("responding")
            .set("kind", s("ToolCall"))
            .set("dispatchStage", s("Streaming"))
            .set("externalOperationId", s("provider-op"))
            .set("detachedOperationId", s("tool-op"))
            .set("detachedExecutor", s("Mcp"))
            .build();
        assert!(decode(&item).is_err());
    }

    #[test]
    fn dispatch_attributes_without_their_stage_are_refused() {
        for item in [
            row("responding")
                .set("kind", s("ToolCall"))
                .set("detachedOperationId", s("op"))
                .set("detachedExecutor", s("Mcp"))
                .build(),
            row("responding")
                .set("externalOperationId", s("provider-op"))
                .build(),
            row("responding")
                .set("dispatchProof", s("ResponseStarted"))
                .build(),
        ] {
            let error = decode(&item).expect_err("partial dispatch evidence is not absence");
            assert!(format!("{error}").contains("dispatchStage"), "{error}");
        }
    }

    #[test]
    fn a_hands_effect_requires_one_well_formed_canonical_generation() {
        let hands = row("prepared").set("kind", s("HandsOperation"));
        assert!(decode(&hands.clone().build()).is_err());
        assert!(decode(&hands.clone().set("generationId", s("generation-7")).build()).is_err());
        let decoded = decode(
            &hands
                .set("generationId", s(generation().to_string()))
                .build(),
        )
        .expect("canonical generation");
        assert_eq!(decoded.generation, Some(generation()));

        let model = row("prepared").set("generationId", s(generation().to_string()));
        assert!(decode(&model.build()).is_err());
    }
}
