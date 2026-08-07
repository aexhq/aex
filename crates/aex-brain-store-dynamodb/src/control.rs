//! Reading the agent control row back as an [`AgentHead`].
//!
//! Decoding never coerces. A missing attribute, a wrong type or an unexpected `itemType` is
//! an error, because a silently defaulted fence or revision is indistinguishable from a
//! correct one right up to the moment it decides who may write.

use aex_brain_app::ports::AgentHead;
use aex_brain_domain::budget::{BudgetNode, DIMENSIONS, DimensionVector};
use aex_brain_domain::ids::{
    AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JournalSeq, Timestamp,
};
use aex_brain_domain::journal::FinishReason;
use aex_session_dynamodb::attr::{CodecError, Item, Row};
use aex_wire::ids::GenerationId;

use crate::plan::{limit_attribute, reserved_attribute, used_attribute};

/// The `itemType` an agent control row declares.
pub const AGENT_CONTROL: &str = aex_session_dynamodb::codec::AGENT_CONTROL;

/// Decodes one control row.
///
/// `open_effects` is supplied separately: the row records a count, and the identities come
/// from the effect partition. Carrying the identities on the control row would make every
/// effect transition rewrite it.
///
/// # Errors
///
/// [`CodecError`] when the row is not an agent control row or an attribute is absent or of
/// the wrong type.
pub fn decode(
    item: &Item,
    key: AgentKey,
    open_effects: Vec<EffectId>,
) -> Result<AgentHead, CodecError> {
    let row = Row::bind(item, AGENT_CONTROL)?;
    let tail = row.u64("journalTail")?;
    let has_journal = row.boolean("hasJournal").unwrap_or(tail > 0);
    let tail_hash = row
        .opt_string("journalTailHash")?
        .map(parse_hash)
        .transpose()?;
    if has_journal != tail_hash.is_some() {
        return Err(CodecError::Malformed {
            item_type: AGENT_CONTROL,
            attribute: "journalTailHash",
            reason: "journal tail sequence and hash must be present together".to_owned(),
        });
    }
    Ok(AgentHead {
        key,
        generation: row.id::<GenerationId>("generationId")?,
        revision: AgentRevision(row.u64("revision")?),
        fence: Fence(row.u64("fence")?),
        journal_tail: has_journal.then_some(JournalSeq(tail)),
        journal_tail_hash: tail_hash,
        cancel_epoch: CancelEpoch(row.opt_u64("cancelEpoch")?.unwrap_or(0)),
        finish: row
            .opt_string("finishReason")?
            .map(parse_finish)
            .transpose()?,
        phase: row.string("status")?.to_owned(),
        budget: decode_budget(&row)?,
        stop_requested: row.boolean("stopRequested").unwrap_or(false),
        open_effects,
        lease_expires_at: row
            .opt_timestamp("leaseExpiresAt")?
            .map_or(Timestamp::from_millis(0), |value| {
                Timestamp::from_millis(value.unix_millis())
            }),
    })
}

fn parse_hash(text: &str) -> Result<ContentHash, CodecError> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CodecError::Malformed {
            item_type: AGENT_CONTROL,
            attribute: "journalTailHash",
            reason: "expected 32 lowercase hexadecimal bytes".to_owned(),
        });
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|_| {
            CodecError::Malformed {
                item_type: AGENT_CONTROL,
                attribute: "journalTailHash",
                reason: "expected 32 lowercase hexadecimal bytes".to_owned(),
            }
        })?;
    }
    Ok(ContentHash(bytes))
}

fn decode_budget(row: &Row<'_>) -> Result<BudgetNode, CodecError> {
    let mut limit = DimensionVector::ZERO;
    let mut reserved = DimensionVector::ZERO;
    let mut used = DimensionVector::ZERO;
    for dimension in DIMENSIONS {
        // Absence is meaningful here and only here: a `DynamoDB` `ADD` creates the
        // attribute on first use, so requiring all twenty-one to be present would make an
        // untouched dimension a decode failure on a freshly created agent.
        limit.set(
            dimension,
            row.opt_u64(limit_attribute(dimension))?.unwrap_or(0),
        );
        reserved.set(
            dimension,
            row.opt_u64(reserved_attribute(dimension))?.unwrap_or(0),
        );
        used.set(
            dimension,
            row.opt_u64(used_attribute(dimension))?.unwrap_or(0),
        );
    }
    Ok(BudgetNode {
        limit,
        reserved,
        used,
        depth: u16::try_from(row.opt_u64("depth")?.unwrap_or(0)).unwrap_or(u16::MAX),
    })
}

fn parse_finish(text: &str) -> Result<FinishReason, CodecError> {
    Ok(match text {
        "completed" => FinishReason::Completed,
        "max_turns" => FinishReason::MaxTurns,
        "max_steps" => FinishReason::MaxSteps,
        "budget" => FinishReason::Budget,
        "timeout" => FinishReason::Timeout,
        "cancelled" => FinishReason::Cancelled,
        "failed" => FinishReason::Failed,
        "interrupted" => FinishReason::Interrupted,
        other => {
            return Err(CodecError::Malformed {
                item_type: AGENT_CONTROL,
                attribute: "finishReason",
                reason: format!("`{other}` is not a finish reason"),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{AGENT_CONTROL, decode};
    use aex_brain_domain::budget::Dimension;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, Fence, JournalSeq, SessionId, Timestamp,
    };
    use aex_brain_domain::journal::FinishReason;
    use aex_session_dynamodb::attr::{ItemBuilder, n, s, stamp};
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1_767_225_600_002, [3; 10]))
    }

    fn key() -> AgentKey {
        AgentKey::new(
            SessionId(uuid::Uuid::from_bytes(
                *Uuid7::compose(1_767_225_600_000, [1; 10]).as_bytes(),
            )),
            AgentId(uuid::Uuid::from_bytes(
                *Uuid7::compose(1_767_225_600_001, [2; 10]).as_bytes(),
            )),
        )
    }

    fn row() -> ItemBuilder {
        let tail_hash = aex_brain_domain::ids::ContentHash::of(b"tail");
        ItemBuilder::new(AGENT_CONTROL)
            .set("generationId", s(generation().to_string()))
            .set("revision", n(7))
            .set("fence", n(3))
            .set("journalTail", n(11))
            .set("journalTailHash", s(tail_hash.to_hex()))
            .set("hasJournal", aex_session_dynamodb::attr::boolean(true))
            .set("cancelEpoch", n(2))
            .set("status", s("awaiting_model"))
            .set(
                "leaseExpiresAt",
                stamp(
                    aex_wire::types::Timestamp::from_unix_millis(1_767_225_615_000)
                        .expect("in range"),
                ),
            )
    }

    #[test]
    fn a_control_row_decodes_into_the_head_a_claim_returns() {
        let head = decode(&row().build(), key(), Vec::new()).expect("a well-formed row");
        assert_eq!(head.revision, AgentRevision(7));
        assert_eq!(head.generation, generation());
        assert_eq!(head.fence, Fence(3));
        assert_eq!(head.journal_tail, Some(JournalSeq(11)));
        assert_eq!(
            head.journal_tail_hash,
            Some(aex_brain_domain::ids::ContentHash::of(b"tail"))
        );
        assert_eq!(head.phase, "awaiting_model");
        assert_eq!(
            head.lease_expires_at,
            Timestamp::from_millis(1_767_225_615_000)
        );
        assert!(!head.stop_requested);
        assert_eq!(head.finish, None);
    }

    /// An agent with no journal yet reports absence, not sequence zero. Conditioning on
    /// zero would let the commit that appended sequence zero be replayed.
    #[test]
    fn an_agent_with_no_journal_reports_absence_rather_than_zero() {
        let item = ItemBuilder::new(AGENT_CONTROL)
            .set("generationId", s(generation().to_string()))
            .set("revision", n(0))
            .set("fence", n(0))
            .set("journalTail", n(0))
            .set("hasJournal", aex_session_dynamodb::attr::boolean(false))
            .set("status", s("queued"))
            .build();
        let head = decode(&item, key(), Vec::new()).expect("a fresh row");
        assert_eq!(head.journal_tail, None);
        assert_eq!(head.journal_tail_hash, None);
    }

    #[test]
    fn a_tail_sequence_without_its_hash_is_refused() {
        let mut item = row().build();
        item.remove("journalTailHash");
        assert!(decode(&item, key(), Vec::new()).is_err());
    }

    #[test]
    fn an_untouched_budget_dimension_decodes_as_zero_rather_than_failing() {
        let head = decode(&row().build(), key(), Vec::new()).expect("a well-formed row");
        for dimension in aex_brain_domain::budget::DIMENSIONS {
            assert_eq!(head.budget.used.get(dimension), 0);
            assert_eq!(head.budget.limit.get(dimension), 0);
        }
    }

    #[test]
    fn a_stored_budget_reads_back_per_dimension() {
        let item = row()
            .set(super::limit_attribute(Dimension::ActiveChildren), n(256))
            .set(super::used_attribute(Dimension::ProviderCalls), n(9))
            .set(super::reserved_attribute(Dimension::CostMicroUsd), n(50))
            .build();
        let head = decode(&item, key(), Vec::new()).expect("a well-formed row");
        assert_eq!(head.budget.limit.get(Dimension::ActiveChildren), 256);
        assert_eq!(head.budget.used.get(Dimension::ProviderCalls), 9);
        assert_eq!(head.budget.reserved.get(Dimension::CostMicroUsd), 50);
    }

    /// A row from another family decodes to nothing rather than to a plausible head.
    #[test]
    fn a_row_of_another_item_type_is_refused() {
        let item = ItemBuilder::new("session_head")
            .set("revision", n(1))
            .build();
        assert!(decode(&item, key(), Vec::new()).is_err());
    }

    #[test]
    fn an_unknown_finish_reason_is_refused_rather_than_defaulted() {
        let item = row().set("finishReason", s("vibes")).build();
        assert!(decode(&item, key(), Vec::new()).is_err());
        let item = row().set("finishReason", s("interrupted")).build();
        let head = decode(&item, key(), Vec::new()).expect("a known reason");
        assert_eq!(head.finish, Some(FinishReason::Interrupted));
    }
}
