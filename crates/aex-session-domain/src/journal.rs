//! The journal envelope, ordering algebra and authority fold.
//!
//! Ownership splits deliberately (D-02): this crate owns the envelope, the
//! ordering algebra and the **authority** fold — status, tail, terminal, the
//! open-effect set, the pending approval — and `aex-brain-domain` owns payload
//! interpretation and the effect prepare/open/complete receipt protocol over the
//! same entries.
//!
//! `fold_control` is pure, total and deterministic. Every guard runs **before**
//! any mutation, so a rejected batch leaves `prior` byte-identical, and
//! `validate_append` shares those guards: the append guard *is* the fold, which
//! is why a journal built through it cannot contain a violation.

use aex_content_domain::ContentDigest;
use aex_internal_contracts::journal::JournalEntryKind;
use aex_wire::ids::{AgentId, ApprovalId};
use aex_wire::types::Timestamp;

use crate::agent::{AgentControl, AgentStatus, AgentTerminal};
use crate::ids::{EffectId, EntryIdentity, JournalSeq};

/// Largest body an entry may carry inline.
pub const INLINE_BODY_MAX_BYTES: u64 = 32_768;

/// Where an entry's payload lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalBody {
    /// Small enough to store beside the envelope.
    Inline(Vec<u8>),
    /// Stored as content and referenced by digest.
    Content(ContentDigest),
}

impl JournalBody {
    /// The inline byte count, when the body is inline.
    #[must_use]
    pub fn inline_len(&self) -> Option<u64> {
        match self {
            Self::Inline(bytes) => Some(bytes.len() as u64),
            Self::Content(_) => None,
        }
    }
}

/// What the authority fold learns from one entry, beyond ordering.
///
/// The payload itself stays opaque here. This is the closed set of *authority*
/// facts an entry may assert; anything else about the payload is Brain's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityFact {
    /// Nothing beyond the ordering position.
    None,
    /// The agent opened an effect.
    EffectOpened(EffectId),
    /// The agent settled an effect.
    EffectSettled(EffectId),
    /// The agent raised an approval and is now parked on it.
    ApprovalRaised(ApprovalId),
    /// The agent's approval resolved.
    ApprovalResolved(ApprovalId),
    /// The agent reached a terminal.
    Terminal(AgentTerminal),
}

/// One journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Which agent wrote it.
    pub agent: AgentId,
    /// Its contiguous position in that agent's journal.
    pub seq: JournalSeq,
    /// Which kind of entry it is.
    pub kind: JournalEntryKind,
    /// Its content-derived identity.
    pub identity: EntryIdentity,
    /// Where its payload lives.
    pub body: JournalBody,
    /// What the authority learns from it.
    pub fact: AuthorityFact,
    /// When the writer recorded it.
    pub recorded_at: Timestamp,
}

/// A contiguous run of entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    /// Which agent.
    pub agent: AgentId,
    /// The first position in the page.
    pub first: JournalSeq,
    /// The entries, in order.
    pub entries: Vec<JournalEntry>,
}

/// Why a journal append was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The batch skipped a position.
    #[error("expected journal position {expected} but found {found}")]
    SequenceGap {
        /// The position that was expected.
        expected: JournalSeq,
        /// The position that was found.
        found: JournalSeq,
    },
    /// The batch went backwards past a position that is not a duplicate.
    #[error("journal tail is {tail} but found {found}")]
    SequenceRegression {
        /// The current tail.
        tail: JournalSeq,
        /// The position that was found.
        found: JournalSeq,
    },
    /// The entry belongs to another agent.
    #[error("entry belongs to another agent")]
    WrongAgent {
        /// The agent being folded.
        expected: AgentId,
        /// The agent the entry names.
        found: AgentId,
    },
    /// Something followed a terminal.
    #[error("entry at {found} follows terminal {terminal:?}")]
    AfterTerminal {
        /// The terminal already recorded.
        terminal: AgentTerminal,
        /// The position that tried to follow it.
        found: JournalSeq,
    },
    /// An inline body exceeded the bound.
    #[error("inline body of {bytes} bytes exceeds the {max} byte bound")]
    InlineBodyTooLarge {
        /// Bytes offered.
        bytes: u64,
        /// The bound.
        max: u64,
    },
    /// An effect was settled that was never opened, or opened twice.
    #[error("effect {effect:?} is unbalanced")]
    UnbalancedEffect {
        /// Which effect.
        effect: EffectId,
    },
}

/// Whether one entry may follow the given control state.
///
/// This is the same guard sequence `fold_control` runs, called out so an append
/// path and a fold path cannot drift.
///
/// # Errors
///
/// Returns the [`JournalError`] naming the first violated rule.
pub fn validate_append(prior: &AgentControl, entry: &JournalEntry) -> Result<(), JournalError> {
    guard(prior, entry).map(|_| ())
}

/// Whether the entry is a duplicate of the tail rather than a new position.
enum Admission {
    /// The entry advances the tail.
    Advance,
    /// The entry repeats one already folded and must be ignored.
    Duplicate,
}

fn guard(prior: &AgentControl, entry: &JournalEntry) -> Result<Admission, JournalError> {
    if entry.agent != prior.id {
        return Err(JournalError::WrongAgent {
            expected: prior.id,
            found: entry.agent,
        });
    }
    if let Some(bytes) = entry.body.inline_len()
        && bytes > INLINE_BODY_MAX_BYTES
    {
        return Err(JournalError::InlineBodyTooLarge {
            bytes,
            max: INLINE_BODY_MAX_BYTES,
        });
    }

    let expected = prior.journal_tail.next();
    if entry.seq.0 < expected.0 {
        // A repeat of the current tail is a legal duplicate only when it is the
        // *same* entry: the identity has to match. A different entry claiming a
        // folded position is a regression, not a retry.
        if entry.seq == prior.journal_tail
            && prior.journal_tail != JournalSeq::INITIAL
            && prior.last_entry == Some(entry.identity)
        {
            return Ok(Admission::Duplicate);
        }
        return Err(JournalError::SequenceRegression {
            tail: prior.journal_tail,
            found: entry.seq,
        });
    }
    if entry.seq != expected {
        return Err(JournalError::SequenceGap {
            expected,
            found: entry.seq,
        });
    }
    if let Some(terminal) = prior.terminal {
        return Err(JournalError::AfterTerminal {
            terminal,
            found: entry.seq,
        });
    }

    match entry.fact {
        AuthorityFact::EffectOpened(effect) => {
            if prior.open_effects.iter().any(|open| *open == effect) {
                return Err(JournalError::UnbalancedEffect { effect });
            }
        }
        AuthorityFact::EffectSettled(effect) => {
            if !prior.open_effects.iter().any(|open| *open == effect) {
                return Err(JournalError::UnbalancedEffect { effect });
            }
        }
        AuthorityFact::None
        | AuthorityFact::ApprovalRaised(_)
        | AuthorityFact::ApprovalResolved(_)
        | AuthorityFact::Terminal(_) => {}
    }
    Ok(Admission::Advance)
}

fn apply(control: &mut AgentControl, entry: &JournalEntry) {
    control.journal_tail = entry.seq;
    control.last_entry = Some(entry.identity);
    control.revision = control.revision.next();
    match entry.fact {
        AuthorityFact::None => {}
        AuthorityFact::EffectOpened(effect) => {
            control.open_effects.open(effect);
        }
        AuthorityFact::EffectSettled(effect) => {
            control.open_effects.close(effect);
        }
        AuthorityFact::ApprovalRaised(approval) => {
            control.pending_approval = Some(approval);
            control.status = AgentStatus::Parked;
        }
        AuthorityFact::ApprovalResolved(_) => {
            control.pending_approval = None;
            if control.status == AgentStatus::Parked {
                control.status = AgentStatus::Running;
            }
        }
        AuthorityFact::Terminal(terminal) => {
            control.terminal = Some(terminal);
            control.status = terminal.status();
            control.claim = None;
            control.pending_approval = None;
            control.queue_reason = None;
        }
    }
}

/// Folds a batch of entries into an agent's authority state.
///
/// Pure, total and deterministic. Every guard runs before any mutation, so a
/// rejected batch leaves `prior` untouched, and
/// `fold(prior, a ++ b) == fold(fold(prior, a), b)` for every split.
///
/// # Errors
///
/// Returns the [`JournalError`] naming the first violated rule.
pub fn fold_control(
    prior: &AgentControl,
    entries: &[JournalEntry],
) -> Result<AgentControl, JournalError> {
    // Guard the whole batch first, against the *evolving* state, on a scratch
    // copy. Nothing is returned unless every entry passes.
    let mut scratch = prior.clone();
    let mut admissions = Vec::with_capacity(entries.len());
    for entry in entries {
        let admission = guard(&scratch, entry)?;
        if matches!(admission, Admission::Advance) {
            apply(&mut scratch, entry);
        }
        admissions.push(admission);
    }
    let _ = admissions;
    Ok(scratch)
}

#[cfg(test)]
mod tests {
    use aex_internal_contracts::journal::JournalEntryKind;
    use aex_wire::ids::{AgentId, PrefixedId as _, Uuid7};

    use super::{
        AuthorityFact, INLINE_BODY_MAX_BYTES, JournalBody, JournalError, fold_control,
        validate_append,
    };
    use crate::agent::{AgentStatus, AgentTerminal};
    use crate::ids::{EntryIdentity, JournalSeq};
    use crate::testing::{child_agent, entry_at};

    #[test]
    fn a_gap_a_regression_and_a_wrong_agent_each_have_their_own_error() {
        let prior = child_agent();
        assert_eq!(
            validate_append(&prior, &entry_at(prior.id, 2, AuthorityFact::None)),
            Err(JournalError::SequenceGap {
                expected: JournalSeq(1),
                found: JournalSeq(2)
            })
        );
        let mut other = entry_at(prior.id, 1, AuthorityFact::None);
        other.agent = AgentId::from_uuid7(Uuid7::compose(9, [9; 10]));
        assert!(matches!(
            validate_append(&prior, &other),
            Err(JournalError::WrongAgent { .. })
        ));

        let advanced =
            fold_control(&prior, &[entry_at(prior.id, 1, AuthorityFact::None)]).expect("folds");
        assert_eq!(
            validate_append(&advanced, &entry_at(prior.id, 0, AuthorityFact::None)),
            Err(JournalError::SequenceRegression {
                tail: JournalSeq(1),
                found: JournalSeq(0)
            })
        );
    }

    #[test]
    fn a_duplicate_identity_collapses_and_does_not_advance_the_tail_twice() {
        let prior = child_agent();
        let first = entry_at(prior.id, 1, AuthorityFact::None);
        let once = fold_control(&prior, std::slice::from_ref(&first)).expect("folds");
        let twice = fold_control(&prior, &[first.clone(), first]).expect("folds");
        assert_eq!(once, twice);
        assert_eq!(twice.journal_tail, JournalSeq(1));
    }

    #[test]
    fn a_rejected_batch_leaves_the_prior_untouched() {
        let prior = child_agent();
        let batch = vec![
            entry_at(prior.id, 1, AuthorityFact::None),
            entry_at(prior.id, 3, AuthorityFact::None),
        ];
        assert!(fold_control(&prior, &batch).is_err());
        assert_eq!(prior, child_agent());
    }

    #[test]
    fn nothing_follows_a_terminal() {
        let prior = child_agent();
        let terminal = fold_control(
            &prior,
            &[entry_at(
                prior.id,
                1,
                AuthorityFact::Terminal(AgentTerminal::Completed),
            )],
        )
        .expect("folds");
        assert_eq!(terminal.status, AgentStatus::Completed);
        assert_eq!(
            fold_control(&terminal, &[entry_at(prior.id, 2, AuthorityFact::None)]),
            Err(JournalError::AfterTerminal {
                terminal: AgentTerminal::Completed,
                found: JournalSeq(2)
            })
        );
    }

    #[test]
    fn an_oversized_inline_body_must_be_content() {
        let prior = child_agent();
        let mut entry = entry_at(prior.id, 1, AuthorityFact::None);
        entry.body = JournalBody::Inline(vec![
            0;
            usize::try_from(INLINE_BODY_MAX_BYTES).expect("fits")
                + 1
        ]);
        assert_eq!(
            validate_append(&prior, &entry),
            Err(JournalError::InlineBodyTooLarge {
                bytes: INLINE_BODY_MAX_BYTES + 1,
                max: INLINE_BODY_MAX_BYTES
            })
        );
    }

    #[test]
    fn the_envelope_kind_comes_from_the_contract_crate() {
        let prior = child_agent();
        let mut entry = entry_at(prior.id, 1, AuthorityFact::None);
        entry.kind = JournalEntryKind::ToolCallBound;
        entry.identity = EntryIdentity::from_bytes([5; 32]);
        assert!(validate_append(&prior, &entry).is_ok());
        assert_eq!(JournalEntryKind::ALL.len(), 22);
    }
}
