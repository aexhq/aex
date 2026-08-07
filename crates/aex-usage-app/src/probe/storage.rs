//! The storage minute cursor, wired to a fenced durable cursor store.
//!
//! The arithmetic itself is pure and lives in
//! `aex_usage_domain::interval::accrue_storage`. This module only does the
//! wiring: read the fenced cursor, apply the transition, and write the new
//! cursor plus any closed fact in **one** transaction. Splitting those two
//! writes would let a crash between them either double-charge a minute or drop
//! one, which is exactly what the cursor exists to prevent.

use std::sync::Arc;

use aex_usage_domain::fact::{Attribution, FactDraft, FactKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, FactId};
use aex_usage_domain::interval::storage::{
    StorageCursor, StorageOwner, StorageSource, StorageTransition, accrue_storage,
};
use aex_usage_domain::meter::Category;
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

use super::{ProbeContext, ProbeError};

/// A durable, fenced storage cursor and the fact that closed with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedCursor {
    /// The cursor state.
    pub cursor: StorageCursor,
    /// The optimistic-concurrency fence the store last wrote.
    pub revision: u64,
}

/// The durable home of a `(owner, generation)` minute cursor.
///
/// The implementation lives in `aex-usage-storage-dynamodb`; the port exists so the
/// accrual logic is testable without a table.
#[async_trait::async_trait]
pub trait StorageCursorStore: std::fmt::Debug + Send + Sync + 'static {
    /// Reads the current cursor for one residence, if it has one.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::CursorStore`] when the read fails.
    async fn read(
        &self,
        workspace: &WorkspaceId,
        owner: &StorageOwner,
    ) -> Result<Option<FencedCursor>, ProbeError>;

    /// Writes the new cursor and the closed fact in one transaction, fenced on
    /// `expected_revision`.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::CursorConflict`] when the fence does not match — a
    /// concurrent transition won, and the caller must re-read rather than
    /// retry blindly — and [`ProbeError::CursorStore`] for anything else.
    async fn commit(
        &self,
        workspace: &WorkspaceId,
        owner: &StorageOwner,
        expected_revision: Option<u64>,
        cursor: &StorageCursor,
        closed: Option<&FactDraft>,
    ) -> Result<Option<FactId>, ProbeError>;
}

/// One authoritative change to one residence.
///
/// Grouped rather than passed as loose parameters so a caller cannot transpose
/// the instant and the commit id, or apply a transition to the wrong owner,
/// without the type system noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageEvent<'a> {
    /// Whose bytes changed.
    pub owner: &'a StorageOwner,
    /// Which durable store holds them.
    pub source: StorageSource,
    /// When the change became authoritative.
    pub at: Timestamp,
    /// What changed.
    pub transition: StorageTransition,
    /// The durable commit that made it authoritative.
    pub commit_id: &'a str,
}

/// Applies authoritative storage transitions against a fenced cursor.
#[derive(Debug)]
pub struct StorageProbe {
    store: Arc<dyn StorageCursorStore>,
}

impl StorageProbe {
    /// Binds a probe to one cursor store.
    #[must_use]
    pub fn new(store: Arc<dyn StorageCursorStore>) -> Self {
        Self { store }
    }

    /// Applies one authoritative transition.
    ///
    /// Returns the identifier of the fact that closed, or `None` when the
    /// elapsed time did not cover a whole minute — the residue carries into the
    /// next segment rather than being charged or dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::Interval`] when the transition is refused by the
    /// accrual rules — a sealed residence, a backwards instant, a second `Put` —
    /// and [`ProbeError::CursorConflict`] when a concurrent transition won.
    pub async fn transition(
        &self,
        context: &ProbeContext,
        attribution: &Attribution,
        event: &StorageEvent<'_>,
    ) -> Result<Option<FactId>, ProbeError> {
        let owner = event.owner;
        let fenced = self.store.read(&context.workspace, owner).await?;
        let (current, revision) = match &fenced {
            Some(found) => (Some(&found.cursor), Some(found.revision)),
            None => (None, None),
        };

        let outcome = accrue_storage(
            current,
            owner,
            event.source,
            event.at,
            event.transition,
            event.commit_id,
        )?;

        let draft = match &outcome.closed {
            Some(measurement) => Some(FactDraft {
                schema_version: SCHEMA_VERSION,
                organization: context.organization.clone(),
                workspace: context.workspace.clone(),
                region: context.region.clone(),
                attribution: attribution.clone(),
                service: context.service.clone(),
                resource: context.resource.clone(),
                authority: AuthorityKey {
                    region: context.region.clone(),
                    category: Category::Storage,
                    kind: AuthorityKind::StorageResidence,
                    authority_id: AuthorityId::parse(&format!(
                        "{}:{}:{}",
                        owner.kind.id(),
                        owner.id.as_str(),
                        owner.generation
                    ))?,
                    segment_ordinal: outcome.segment_ordinal,
                },
                pricing_version: context.pricing_version.clone(),
                reservation: context.reservation.clone(),
                kind: FactKind::Measured(measurement.clone()),
            }),
            None => None,
        };

        self.store
            .commit(
                &context.workspace,
                owner,
                revision,
                &outcome.cursor,
                draft.as_ref(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{FencedCursor, StorageCursorStore, StorageEvent, StorageProbe};
    use crate::probe::ProbeError;
    use crate::probe::testing::{at, probe_context};
    use aex_usage_domain::fact::{Attribution, FactDraft};
    use aex_usage_domain::identity::{AuthorityId, FactId};
    use aex_usage_domain::interval::storage::{
        StorageCursor, StorageOwner, StorageOwnerKind, StorageSource, StorageTransition,
    };
    use aex_usage_domain::wire_pending::WorkspaceId;
    use std::sync::{Arc, Mutex};

    /// One recorded commit: the fence it named, the cursor it wrote and the
    /// fact it closed with.
    type CommitLog = Vec<(Option<u64>, StorageCursor, Option<FactId>)>;

    /// An in-memory cursor store that records every commit it was asked for.
    #[derive(Debug, Default)]
    struct FakeStore {
        state: Mutex<Option<FencedCursor>>,
        commits: Mutex<CommitLog>,
        conflict_next: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl StorageCursorStore for FakeStore {
        async fn read(
            &self,
            _workspace: &WorkspaceId,
            _owner: &StorageOwner,
        ) -> Result<Option<FencedCursor>, ProbeError> {
            Ok(self.state.lock().expect("lock").clone())
        }

        async fn commit(
            &self,
            _workspace: &WorkspaceId,
            _owner: &StorageOwner,
            expected_revision: Option<u64>,
            cursor: &StorageCursor,
            closed: Option<&FactDraft>,
        ) -> Result<Option<FactId>, ProbeError> {
            if std::mem::replace(&mut *self.conflict_next.lock().expect("lock"), false) {
                return Err(ProbeError::CursorConflict {
                    expected: expected_revision,
                });
            }
            let fact_id = closed.map(FactDraft::fact_id);
            self.commits.lock().expect("lock").push((
                expected_revision,
                cursor.clone(),
                fact_id.clone(),
            ));
            let next_revision = expected_revision.map_or(0, |revision| revision + 1);
            *self.state.lock().expect("lock") = Some(FencedCursor {
                cursor: cursor.clone(),
                revision: next_revision,
            });
            Ok(fact_id)
        }
    }

    fn owner() -> StorageOwner {
        StorageOwner {
            kind: StorageOwnerKind::ContentObject,
            id: AuthorityId::parse("obj-1").expect("id"),
            generation: 4,
        }
    }

    async fn apply(
        probe: &StorageProbe,
        millis: i64,
        transition: StorageTransition,
        commit: &str,
    ) -> Result<Option<FactId>, ProbeError> {
        probe
            .transition(
                &probe_context(),
                &Attribution::default(),
                &StorageEvent {
                    owner: &owner(),
                    source: StorageSource::S3,
                    at: at(millis),
                    transition,
                    commit_id: commit,
                },
            )
            .await
    }

    #[tokio::test]
    async fn opening_a_residence_writes_a_cursor_and_no_fact() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        let closed = apply(&probe, 0, StorageTransition::Put { bytes: 4_096 }, "c0")
            .await
            .expect("opens");
        assert!(closed.is_none(), "opening charges nothing");

        let commits = store.commits.lock().expect("lock");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, None, "the first write is unfenced");
        assert_eq!(commits[0].1.bytes, 4_096);
    }

    #[tokio::test]
    async fn a_sub_minute_interior_transition_carries_its_residue_forward() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        apply(&probe, 0, StorageTransition::Put { bytes: 1_024 }, "c0")
            .await
            .expect("opens");
        // 30 seconds: less than a whole minute, so nothing closes.
        let closed = apply(
            &probe,
            30_000,
            StorageTransition::Resize { bytes: 2_048 },
            "c1",
        )
        .await
        .expect("resizes");
        assert!(closed.is_none(), "no whole minute elapsed");

        // 90 s total: one whole minute is now chargeable.
        let closed = apply(
            &probe,
            90_000,
            StorageTransition::Resize { bytes: 2_048 },
            "c2",
        )
        .await
        .expect("resizes");
        assert!(closed.is_some(), "the carried residue completed a minute");
    }

    #[tokio::test]
    async fn the_cursor_and_the_closed_fact_are_committed_together() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        apply(&probe, 0, StorageTransition::Put { bytes: 8_192 }, "c0")
            .await
            .expect("opens");
        apply(&probe, 120_000, StorageTransition::HardDelete, "c1")
            .await
            .expect("deletes");

        let commits = store.commits.lock().expect("lock");
        let terminal = commits.last().expect("a terminal commit");
        assert!(terminal.1.sealed, "a hard delete seals the cursor");
        assert!(
            terminal.2.is_some(),
            "the sealed cursor and its closing fact are one transaction"
        );
    }

    #[tokio::test]
    async fn every_write_after_the_first_is_fenced_on_the_prior_revision() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        apply(&probe, 0, StorageTransition::Put { bytes: 512 }, "c0")
            .await
            .expect("opens");
        apply(&probe, 60_000, StorageTransition::Trash, "c1")
            .await
            .expect("trashes");

        let commits = store.commits.lock().expect("lock");
        assert_eq!(commits[0].0, None);
        assert_eq!(commits[1].0, Some(0), "the second write names the fence");
    }

    #[tokio::test]
    async fn a_lost_fence_race_surfaces_rather_than_being_retried_blindly() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        apply(&probe, 0, StorageTransition::Put { bytes: 512 }, "c0")
            .await
            .expect("opens");
        *store.conflict_next.lock().expect("lock") = true;

        let refused = apply(
            &probe,
            60_000,
            StorageTransition::Resize { bytes: 1_024 },
            "c1",
        )
        .await;
        assert!(matches!(refused, Err(ProbeError::CursorConflict { .. })));
    }

    #[tokio::test]
    async fn a_transition_without_a_residence_is_refused() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(store as Arc<dyn StorageCursorStore>);

        let refused = apply(&probe, 0, StorageTransition::Resize { bytes: 1 }, "c0").await;
        assert!(matches!(refused, Err(ProbeError::Interval(_))));
    }

    #[tokio::test]
    async fn a_sealed_residence_refuses_every_later_transition() {
        let store = Arc::new(FakeStore::default());
        let probe = StorageProbe::new(Arc::clone(&store) as Arc<dyn StorageCursorStore>);

        apply(&probe, 0, StorageTransition::Put { bytes: 512 }, "c0")
            .await
            .expect("opens");
        apply(&probe, 60_000, StorageTransition::HardDelete, "c1")
            .await
            .expect("deletes");

        let refused = apply(&probe, 90_000, StorageTransition::Restore, "c2").await;
        assert!(matches!(refused, Err(ProbeError::Interval(_))));
    }
}
