//! Bounded reconstruction from the authoritative journal or one verified checkpoint.
//!
//! A warm process may reuse an exact-revision fold cache entry. A cold activation loads the
//! latest committed immutable checkpoint and only the journal tail after its exact boundary;
//! agents without a checkpoint retain the bounded sequence-zero bootstrap path.

use crate::activation::{ActivationError, RestoreBudget};
use crate::ports::{JournalCursor, JournalStore, ReadBudget, StoreError};
use aex_brain_domain::checkpoint::ContextCheckpoint;
use aex_brain_domain::fold::{FoldState, apply};
use aex_brain_domain::ids::{AgentKey, ContentHash, JournalSeq};

/// How a fold was reconstructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreSource {
    /// An exact-revision process-local fold passed the claimed-tail check.
    WarmCache,
    /// The bounded authoritative journal fit from sequence zero.
    JournalFromZero,
    /// A verified immutable baseline plus entries after its covered sequence.
    Checkpoint {
        /// Last sequence represented by the S3 baseline.
        covers_through: JournalSeq,
    },
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

/// Reconstructs one claimed agent from journal sequence zero when no checkpoint exists.
///
/// Every success compares the folded `(sequence, hash)` with the head returned by the fenced
/// claim. If the complete authoritative history does not fit, the activation refuses with
/// [`StoreError::RestoreBudgetExhausted`] until a checkpoint is committed.
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
        None,
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

/// Reconstructs from one verified immutable baseline and only its recent tail.
///
/// The checkpoint's fold carries all execution projections. Its retained hash
/// window begins after `covers_through`, so applying the tail is bounded and no
/// original journal row is changed or deleted.
///
/// # Errors
///
/// Returns [`ActivationError`] when the claim is already behind the checkpoint
/// boundary, or when the bounded tail cannot be decoded, folded, or does not
/// reach the exact claimed tail.
pub async fn restore_from_checkpoint(
    journal: &dyn JournalStore,
    checkpoint: ContextCheckpoint,
    claimed_seq: Option<JournalSeq>,
    claimed_hash: Option<ContentHash>,
    page: ReadBudget,
    total: RestoreBudget,
) -> Result<RestoredFold, ActivationError> {
    let covers = checkpoint.covers_through;
    if claimed_seq.is_some_and(|tail| covers > tail) {
        return Err(StoreError::JournalTailMismatch {
            claimed_seq,
            claimed_hash,
            folded_seq: Some(covers),
            folded_hash: Some(checkpoint.covers_hash),
        }
        .into());
    }
    let checkpoint_bytes = serde_json::to_vec(&checkpoint)
        .map_err(|_| crate::ports::CheckpointError::Corrupt)?
        .len();
    let (state, _, tail_bytes) = read_from(
        journal,
        checkpoint.key,
        checkpoint.state,
        covers.next(),
        claimed_seq,
        claimed_hash,
        Some(checkpoint.covers_hash),
        page,
        total,
    )
    .await?;
    Ok(RestoredFold {
        state,
        source: RestoreSource::Checkpoint {
            covers_through: covers,
        },
        retained_bytes: checkpoint_bytes.saturating_add(tail_bytes),
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
    baseline_hash: Option<ContentHash>,
    page: ReadBudget,
    total: RestoreBudget,
) -> Result<(FoldState, usize, usize), ActivationError> {
    let mut cursor: Option<JournalCursor> = None;
    let mut restored_entries = 0_usize;
    let mut restored_bytes = 0_usize;

    if state.tail == claimed_seq && state.hashes.last().copied().or(baseline_hash) == claimed_hash {
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

    let folded_hash = state.hashes.last().copied().or(baseline_hash);
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
    use std::sync::Mutex;

    use super::{page_exhaustion_is_total, restore_from_checkpoint};
    use crate::activation::RestoreBudget;
    use crate::ports::{
        BoxFuture, CommitError, CommitReceipt, DecisionContext, JournalCursor, JournalPage,
        JournalStore, ReadBudget, StoreError,
    };
    use aex_brain_domain::checkpoint::{CHECKPOINT_SCHEMA_VERSION, ContextCheckpoint};
    use aex_brain_domain::commit::DecisionCommit;
    use aex_brain_domain::fold::fold;
    use aex_brain_domain::ids::{AgentId, AgentKey, ContentHash, JournalSeq, SessionId, Timestamp};
    use aex_brain_test_support::journal_gen::{HistoryBuilder, grant, started, user_text};
    use uuid::Uuid;

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        match core::future::Future::poll(future.as_mut(), &mut context) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("the restore fixture must be immediately ready"),
        }
    }

    #[derive(Debug)]
    struct TailJournal {
        key: AgentKey,
        original: Vec<aex_brain_domain::journal::JournalEntry>,
        requested_from: Mutex<Vec<JournalSeq>>,
    }

    impl JournalStore for TailJournal {
        fn load_head<'a>(
            &'a self,
            _key: &'a AgentKey,
        ) -> BoxFuture<'a, Result<Option<crate::ports::AgentHead>, StoreError>> {
            Box::pin(async { unreachable!("restore does not load a second head") })
        }

        fn read_page<'a>(
            &'a self,
            key: &'a AgentKey,
            from: JournalSeq,
            _budget: ReadBudget,
            after: Option<JournalCursor>,
        ) -> BoxFuture<'a, Result<JournalPage, StoreError>> {
            Box::pin(async move {
                assert_eq!(*key, self.key);
                assert!(after.is_none());
                self.requested_from.lock().expect("not poisoned").push(from);
                let entries: Vec<_> = self
                    .original
                    .iter()
                    .filter(|entry| entry.envelope.seq >= from)
                    .cloned()
                    .collect();
                let hydrated_bytes = entries
                    .iter()
                    .map(|entry| serde_json::to_vec(entry).expect("fixture serializes").len())
                    .sum();
                Ok(JournalPage {
                    entries,
                    hydrated_bytes,
                    next: None,
                })
            })
        }

        fn commit<'a>(
            &'a self,
            _context: &'a DecisionContext,
            _commit: &'a DecisionCommit,
        ) -> BoxFuture<'a, Result<CommitReceipt, CommitError>> {
            Box::pin(async { unreachable!("restore is read-only") })
        }
    }

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

    #[test]
    fn a_new_owner_reads_only_the_tail_and_the_original_journal_stays_intact() {
        let key = AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)));
        let original = HistoryBuilder::new()
            .push(started(grant(1_000)))
            .push(user_text("after the checkpoint"))
            .build();
        let original_bytes = serde_json::to_vec(&original).expect("fixture serializes");
        let mut baseline = fold(&original[..1]).expect("the checkpoint prefix folds");
        let covers_hash = baseline.hashes[0];
        baseline.hashes.clear();
        baseline.base_seq = JournalSeq(1);
        let checkpoint = ContextCheckpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            key,
            id: ContentHash::of(b"checkpoint"),
            previous: None,
            covers_through: JournalSeq::ZERO,
            covers_hash,
            source_hash: ContentHash::of(b"source"),
            approximate_tokens: 1,
            compactor: "test".to_owned(),
            created_at: Timestamp(1),
            state: baseline,
        };
        let journal = TailJournal {
            key,
            original: original.clone(),
            requested_from: Mutex::new(Vec::new()),
        };
        let claimed_hash = original.last().map(|entry| entry.envelope.content_hash);

        let restored = block_on(restore_from_checkpoint(
            &journal,
            checkpoint,
            Some(JournalSeq(1)),
            claimed_hash,
            ReadBudget {
                max_entries: 8,
                max_bytes: 64 * 1_024,
            },
            RestoreBudget {
                max_entries: 8,
                max_bytes: 64 * 1_024,
            },
        ))
        .expect("checkpoint plus tail restores");

        assert_eq!(
            journal
                .requested_from
                .lock()
                .expect("not poisoned")
                .as_slice(),
            &[JournalSeq(1)],
            "sequence zero must not be hydrated again"
        );
        let mut expected = fold(&original).expect("the full journal folds");
        expected.base_seq = JournalSeq(1);
        expected.hashes = vec![claimed_hash.expect("the fixture has a tail hash")];
        assert_eq!(restored.state, expected);
        assert_eq!(
            serde_json::to_vec(&journal.original).expect("fixture serializes"),
            original_bytes,
            "restoration cannot rewrite the authoritative journal"
        );
    }
}
