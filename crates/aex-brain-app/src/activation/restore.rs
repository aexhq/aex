//! Bounded reconstruction from the authoritative journal.
//!
//! Brain has no durable fold snapshot or filesystem recovery authority. A warm process may
//! reuse an exact-revision fold cache entry, but every cold activation reconstructs from
//! journal sequence zero under one explicit aggregate entry/byte ceiling.

use crate::activation::{ActivationError, RestoreBudget};
use crate::ports::{JournalCursor, JournalStore, ReadBudget, StoreError};
use aex_brain_domain::fold::{FoldState, apply};
use aex_brain_domain::ids::{AgentKey, ContentHash, JournalSeq};

/// How a fold was reconstructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreSource {
    /// An exact-revision process-local fold passed the claimed-tail check.
    WarmCache,
    /// The bounded authoritative journal fit from sequence zero.
    JournalFromZero,
}

/// A state that proved it reaches the exact head returned by the fenced claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredFold {
    /// Validated fold state.
    pub state: FoldState,
    /// Which path produced it.
    pub source: RestoreSource,
    /// Canonical authoritative bytes retained to derive this fold.
    ///
    /// This is the cache accounting input, not a claim about allocator RSS. Activation
    /// admission separately reserves the measured peak resident restore bound before any
    /// journal page is hydrated.
    pub retained_bytes: usize,
}

/// Reconstructs one claimed agent from journal sequence zero.
///
/// Every success compares the folded `(sequence, hash)` with the head returned by the fenced
/// claim. There is deliberately no durable snapshot fallback: if the complete authoritative
/// history does not fit, the activation refuses with [`StoreError::RestoreBudgetExhausted`].
///
/// # Errors
///
/// Returns [`ActivationError`] if the bounded journal cannot be decoded and folded or does
/// not reach the exact claimed tail.
pub async fn restore(
    journal: &dyn JournalStore,
    key: AgentKey,
    claimed_seq: Option<JournalSeq>,
    claimed_hash: Option<ContentHash>,
    page: ReadBudget,
    total: RestoreBudget,
) -> Result<RestoredFold, ActivationError> {
    let (state, _, retained_bytes) = read_from(
        journal,
        key,
        FoldState::empty(),
        JournalSeq::ZERO,
        claimed_seq,
        claimed_hash,
        page,
        total,
    )
    .await?;
    Ok(RestoredFold {
        state,
        source: RestoreSource::JournalFromZero,
        retained_bytes,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the exact claimed tail and both independent page/aggregate bounds are one restore proof"
)]
async fn read_from(
    journal: &dyn JournalStore,
    key: AgentKey,
    mut state: FoldState,
    mut from: JournalSeq,
    claimed_seq: Option<JournalSeq>,
    claimed_hash: Option<ContentHash>,
    page: ReadBudget,
    total: RestoreBudget,
) -> Result<(FoldState, usize, usize), ActivationError> {
    let mut cursor: Option<JournalCursor> = None;
    let mut restored_entries = 0_usize;
    let mut restored_bytes = 0_usize;

    if state.tail == claimed_seq && state.hashes.last().copied() == claimed_hash {
        return Ok((state, 0, 0));
    }

    loop {
        let remaining_entries = total.max_entries.saturating_sub(restored_entries);
        let remaining_bytes = total.max_bytes.saturating_sub(restored_bytes);
        if remaining_entries == 0 || remaining_bytes == 0 {
            return Err(StoreError::RestoreBudgetExhausted {
                entries: restored_entries.saturating_add(usize::from(remaining_entries == 0)),
                bytes: restored_bytes.saturating_add(usize::from(remaining_bytes == 0)),
                max_entries: total.max_entries,
                max_bytes: total.max_bytes,
            }
            .into());
        }
        let page_budget = ReadBudget {
            max_entries: page.max_entries.min(remaining_entries),
            max_bytes: page.max_bytes.min(remaining_bytes),
        };
        let next_page = match journal
            .read_page(&key, from, page_budget, cursor.take())
            .await
        {
            Ok(page) => page,
            Err(StoreError::ReadBudgetExhausted { entries, bytes })
                if page_exhaustion_is_total(remaining_entries, remaining_bytes, page) =>
            {
                return Err(StoreError::RestoreBudgetExhausted {
                    entries: restored_entries.saturating_add(entries),
                    bytes: restored_bytes.saturating_add(bytes),
                    max_entries: total.max_entries,
                    max_bytes: total.max_bytes,
                }
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        let next_entries = restored_entries.saturating_add(next_page.entries.len());
        let next_bytes = restored_bytes.saturating_add(next_page.hydrated_bytes);
        if next_entries > total.max_entries || next_bytes > total.max_bytes {
            return Err(StoreError::RestoreBudgetExhausted {
                entries: next_entries,
                bytes: next_bytes,
                max_entries: total.max_entries,
                max_bytes: total.max_bytes,
            }
            .into());
        }
        for entry in &next_page.entries {
            apply(&mut state, entry)?;
        }
        restored_entries = next_entries;
        restored_bytes = next_bytes;
        match next_page.next {
            Some(next) => {
                if next_page.entries.is_empty() {
                    return Err(StoreError::Undecodable {
                        location: "journal continuation".to_owned(),
                        reason: "a continuation followed a page with no journal entries".to_owned(),
                    }
                    .into());
                }
                from = next.next();
                cursor = Some(next);
            }
            None => break,
        }
    }

    let folded_hash = state.hashes.last().copied();
    if state.tail != claimed_seq || folded_hash != claimed_hash {
        return Err(StoreError::JournalTailMismatch {
            claimed_seq,
            claimed_hash,
            folded_seq: state.tail,
            folded_hash,
        }
        .into());
    }
    Ok((state, restored_entries, restored_bytes))
}

const fn page_exhaustion_is_total(
    remaining_entries: usize,
    remaining_bytes: usize,
    page: ReadBudget,
) -> bool {
    remaining_entries <= page.max_entries || remaining_bytes <= page.max_bytes
}

#[cfg(test)]
mod tests {
    use super::page_exhaustion_is_total;
    use crate::ports::ReadBudget;

    #[test]
    fn either_aggregate_dimension_classifies_a_page_exhaustion() {
        let page = ReadBudget {
            max_entries: 256,
            max_bytes: 1_024,
        };
        assert!(page_exhaustion_is_total(256, 2_048, page));
        assert!(page_exhaustion_is_total(512, 1_024, page));
        assert!(!page_exhaustion_is_total(512, 2_048, page));
    }
}
