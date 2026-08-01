//! Workspaces.
//!
//! A workspace is a region-pinned execution and content boundary. Its region and
//! its organization are immutable — enforced here and by a `BEFORE UPDATE`
//! trigger — and a `provisioning` workspace is invisible to every public read,
//! so a caller never sees a workspace whose regional half is not durable yet.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::Revision;
use crate::operation::Fence;
use crate::slug::Slug;

/// The lifecycle of a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkspaceStatus {
    /// Central row committed, regional half not durable yet. Never visible.
    Provisioning,
    /// Live.
    Active,
    /// Deletion accepted; admission is already closed.
    Deleting,
    /// Deleted. The row survives as a tombstone.
    Deleted,
}

impl WorkspaceStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 4] = [
        Self::Provisioning,
        Self::Active,
        Self::Deleting,
        Self::Deleted,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Active => "active",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// A workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// Public id payload.
    pub id: Uuid,
    /// Owning organization. Immutable.
    pub organization_id: Uuid,
    /// Display name.
    pub name: String,
    /// Slug, unique inside the organization among non-deleted workspaces.
    pub slug: Slug,
    /// Placement. Immutable.
    pub region: aex_wire::types::Region,
    /// Lifecycle.
    pub status: WorkspaceStatus,
    /// The internal operation that provisioned it.
    pub provision_operation_id: Uuid,
    /// The fence that operation advanced under.
    pub provision_fence: Fence,
    /// The public operation deleting it, once accepted.
    pub deletion_operation_id: Option<Uuid>,
    /// The fence that operation advanced under.
    pub deletion_fence: Option<Fence>,
    /// Optimistic-concurrency revision.
    pub revision: Revision,
    /// When the central row was committed.
    pub created_at: OffsetDateTime,
    /// When the row last changed.
    pub updated_at: OffsetDateTime,
    /// When both halves became durable.
    pub activated_at: Option<OffsetDateTime>,
    /// When the regional half finished being removed.
    pub deleted_at: Option<OffsetDateTime>,
    /// Who created it.
    pub created_by_user_id: Uuid,
}

/// Why a workspace transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceTransition {
    /// The workspace is not in a state this transition applies to.
    #[error("a `{from}` workspace cannot become `{to}`", from = .from.as_str(), to = .to.as_str())]
    WrongStatus {
        /// Where it was.
        from: WorkspaceStatus,
        /// Where the caller wanted to take it.
        to: WorkspaceStatus,
    },
    /// The supplied fence is not ahead of the recorded one.
    #[error("fence {supplied} does not advance past {recorded}")]
    StaleFence {
        /// What the caller supplied.
        supplied: u64,
        /// What the row already holds.
        recorded: u64,
    },
}

impl Workspace {
    /// The longest accepted display name.
    pub const MAX_NAME_LEN: usize = 128;

    /// Whether any public read may return this workspace.
    ///
    /// A `provisioning` workspace is hidden because its regional half may never
    /// become durable; returning it would let a caller address a workspace that
    /// does not exist anywhere but here.
    #[must_use]
    pub const fn is_publicly_visible(&self) -> bool {
        !matches!(self.status, WorkspaceStatus::Provisioning)
    }

    /// Whether the workspace still admits new work.
    #[must_use]
    pub const fn admits_work(&self) -> bool {
        matches!(self.status, WorkspaceStatus::Active)
    }

    /// The regional API host for this workspace.
    #[must_use]
    pub fn api_host(&self) -> String {
        format!("{}.api.aex.dev", self.region.as_str())
    }

    /// Marks both halves durable.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceTransition::WrongStatus`] unless the workspace is
    /// `provisioning`, and [`WorkspaceTransition::StaleFence`] for a fence that
    /// does not advance.
    pub fn activate(&self, fence: Fence, at: OffsetDateTime) -> Result<Self, WorkspaceTransition> {
        if self.status != WorkspaceStatus::Provisioning {
            return Err(WorkspaceTransition::WrongStatus {
                from: self.status,
                to: WorkspaceStatus::Active,
            });
        }
        check_fence(self.provision_fence, fence)?;
        Ok(Self {
            status: WorkspaceStatus::Active,
            provision_fence: fence,
            activated_at: Some(at),
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Accepts a deletion, closing admission immediately.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceTransition::WrongStatus`] unless the workspace is
    /// `active`.
    pub fn begin_deletion(
        &self,
        operation_id: Uuid,
        fence: Fence,
        at: OffsetDateTime,
    ) -> Result<Self, WorkspaceTransition> {
        if self.status != WorkspaceStatus::Active {
            return Err(WorkspaceTransition::WrongStatus {
                from: self.status,
                to: WorkspaceStatus::Deleting,
            });
        }
        Ok(Self {
            status: WorkspaceStatus::Deleting,
            deletion_operation_id: Some(operation_id),
            deletion_fence: Some(fence),
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }

    /// Records that the regional half is gone.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceTransition::WrongStatus`] unless the workspace is
    /// `deleting`, and [`WorkspaceTransition::StaleFence`] for a fence that does
    /// not advance past the one deletion was accepted under.
    pub fn complete_deletion(
        &self,
        fence: Fence,
        at: OffsetDateTime,
    ) -> Result<Self, WorkspaceTransition> {
        if self.status != WorkspaceStatus::Deleting {
            return Err(WorkspaceTransition::WrongStatus {
                from: self.status,
                to: WorkspaceStatus::Deleted,
            });
        }
        let recorded = self.deletion_fence.unwrap_or(Fence::FIRST);
        check_fence(recorded, fence)?;
        Ok(Self {
            status: WorkspaceStatus::Deleted,
            deletion_fence: Some(fence),
            deleted_at: Some(at),
            revision: self.revision.next(),
            updated_at: at,
            ..self.clone()
        })
    }
}

/// Checks that `supplied` is at least `recorded`.
fn check_fence(recorded: Fence, supplied: Fence) -> Result<(), WorkspaceTransition> {
    if supplied.get() < recorded.get() {
        return Err(WorkspaceTransition::StaleFence {
            supplied: supplied.get(),
            recorded: recorded.get(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Workspace, WorkspaceStatus, WorkspaceTransition};
    use crate::Revision;
    use crate::operation::Fence;
    use crate::slug::Slug;
    use aex_wire::types::Region;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn workspace(status: WorkspaceStatus) -> Workspace {
        Workspace {
            id: Uuid::from_u128(1),
            organization_id: Uuid::from_u128(2),
            name: "Prod".to_owned(),
            slug: Slug::parse("prod").expect("a valid slug"),
            region: Region::EuWest1,
            status,
            provision_operation_id: Uuid::from_u128(3),
            provision_fence: Fence::FIRST,
            deletion_operation_id: None,
            deletion_fence: None,
            revision: Revision::INITIAL,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            activated_at: None,
            deleted_at: None,
            created_by_user_id: Uuid::from_u128(4),
        }
    }

    #[test]
    fn a_provisioning_workspace_is_invisible_and_admits_no_work() {
        let provisioning = workspace(WorkspaceStatus::Provisioning);
        assert!(!provisioning.is_publicly_visible());
        assert!(!provisioning.admits_work());
    }

    #[test]
    fn every_other_status_is_visible_and_only_active_admits_work() {
        for status in [
            WorkspaceStatus::Active,
            WorkspaceStatus::Deleting,
            WorkspaceStatus::Deleted,
        ] {
            let workspace = workspace(status);
            assert!(workspace.is_publicly_visible(), "{status:?}");
            assert_eq!(
                workspace.admits_work(),
                status == WorkspaceStatus::Active,
                "{status:?}"
            );
        }
    }

    #[test]
    fn the_lifecycle_runs_in_exactly_one_direction() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let active = workspace(WorkspaceStatus::Provisioning)
            .activate(Fence::FIRST, at)
            .expect("provisioning activates");
        assert_eq!(active.status, WorkspaceStatus::Active);
        assert_eq!(active.activated_at, Some(at));

        assert!(matches!(
            active.activate(Fence::FIRST, at),
            Err(WorkspaceTransition::WrongStatus { .. })
        ));

        let deleting = active
            .begin_deletion(Uuid::from_u128(9), Fence::FIRST, at)
            .expect("active begins deletion");
        assert_eq!(deleting.status, WorkspaceStatus::Deleting);
        assert!(!deleting.admits_work());

        let deleted = deleting
            .complete_deletion(Fence::FIRST.advance(), at)
            .expect("deleting completes");
        assert_eq!(deleted.status, WorkspaceStatus::Deleted);
        assert_eq!(deleted.deleted_at, Some(at));

        assert!(matches!(
            deleted.begin_deletion(Uuid::from_u128(10), Fence::FIRST, at),
            Err(WorkspaceTransition::WrongStatus { .. })
        ));
        assert!(matches!(
            deleted.complete_deletion(Fence::FIRST, at),
            Err(WorkspaceTransition::WrongStatus { .. })
        ));
    }

    #[test]
    fn a_stale_fence_is_rejected_rather_than_merged() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let mut provisioning = workspace(WorkspaceStatus::Provisioning);
        provisioning.provision_fence = Fence::new(5);
        assert_eq!(
            provisioning.activate(Fence::new(4), at),
            Err(WorkspaceTransition::StaleFence {
                supplied: 4,
                recorded: 5
            })
        );
        assert!(provisioning.activate(Fence::new(5), at).is_ok());
    }

    #[test]
    fn placement_is_carried_through_every_transition() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let before = workspace(WorkspaceStatus::Provisioning);
        let after = before.activate(Fence::FIRST, at).expect("activates");
        assert_eq!(after.region, before.region);
        assert_eq!(after.organization_id, before.organization_id);
    }

    #[test]
    fn the_api_host_is_derived_from_the_region() {
        assert_eq!(
            workspace(WorkspaceStatus::Active).api_host(),
            "eu-west-1.api.aex.dev"
        );
    }

    #[test]
    fn the_status_spellings_round_trip() {
        for status in WorkspaceStatus::ALL {
            assert_eq!(WorkspaceStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(WorkspaceStatus::parse("archived"), None);
    }
}
