//! Deterministic central-plane fixtures.
//!
//! Every value is derived from the injected clock, the seeded identifier factory
//! and the run prefix, so two runs with the same seed produce identical inputs.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::TestClock;
use crate::ids::IdFactory;
use crate::prefix::{PrefixError, RunPrefix};

/// An organization as the control schema would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationFixture {
    /// Organization identifier.
    pub id: Uuid,
    /// Synthetic slug inside the run namespace.
    pub slug: String,
    /// Creation instant read from the injected clock.
    pub created_at: OffsetDateTime,
}

/// A workspace as the control schema would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFixture {
    /// Workspace identifier.
    pub id: Uuid,
    /// Owning organization.
    pub organization_id: Uuid,
    /// Synthetic slug inside the run namespace.
    pub slug: String,
    /// Region the workspace is placed in.
    pub region: String,
    /// Creation instant read from the injected clock.
    pub created_at: OffsetDateTime,
}

/// An API key as the control schema would hold it. No fixture ever carries a
/// usable secret; only the public prefix and the epoch are modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyFixture {
    /// Key identifier.
    pub id: Uuid,
    /// Owning workspace.
    pub workspace_id: Uuid,
    /// Public, non-secret key prefix.
    pub public_prefix: String,
    /// Revocation epoch this key was issued under.
    pub epoch: u64,
    /// Creation instant read from the injected clock.
    pub created_at: OffsetDateTime,
}

/// Builds central-plane fixtures from one clock, one seed and one run prefix.
#[derive(Debug, Clone)]
pub struct CentralFixtures {
    clock: TestClock,
    ids: IdFactory,
    prefix: RunPrefix,
    issued: u64,
}

impl CentralFixtures {
    /// A fixture factory bound to `prefix` and `seed`.
    #[must_use]
    pub fn new(prefix: RunPrefix, seed: u64) -> Self {
        Self {
            clock: TestClock::new(),
            ids: IdFactory::new(seed),
            prefix,
            issued: 0,
        }
    }

    /// The injected clock, so a test can advance it between fixtures.
    #[must_use]
    pub const fn clock(&self) -> &TestClock {
        &self.clock
    }

    /// Mutable access to the injected clock.
    pub const fn clock_mut(&mut self) -> &mut TestClock {
        &mut self.clock
    }

    /// The run prefix every synthetic name is built from.
    #[must_use]
    pub const fn prefix(&self) -> &RunPrefix {
        &self.prefix
    }

    /// A fresh organization.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated name.
    pub fn organization(&mut self) -> Result<OrganizationFixture, PrefixError> {
        let slug = self.next_name("org")?;
        Ok(OrganizationFixture {
            id: self.ids.next_id(),
            slug,
            created_at: self.clock.tick(),
        })
    }

    /// A fresh workspace inside `organization_id`, placed in `region`.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated name.
    pub fn workspace(
        &mut self,
        organization_id: Uuid,
        region: &str,
    ) -> Result<WorkspaceFixture, PrefixError> {
        let slug = self.next_name("ws")?;
        Ok(WorkspaceFixture {
            id: self.ids.next_id(),
            organization_id,
            slug,
            region: region.to_owned(),
            created_at: self.clock.tick(),
        })
    }

    /// A fresh API key for `workspace_id` at `epoch`.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated name.
    pub fn api_key(
        &mut self,
        workspace_id: Uuid,
        epoch: u64,
    ) -> Result<ApiKeyFixture, PrefixError> {
        let public_prefix = self.next_name("key")?;
        Ok(ApiKeyFixture {
            id: self.ids.next_id(),
            workspace_id,
            public_prefix,
            epoch,
            created_at: self.clock.tick(),
        })
    }

    fn next_name(&mut self, kind: &str) -> Result<String, PrefixError> {
        let ordinal = self.issued;
        self.issued = self.issued.saturating_add(1);
        self.prefix.resource(&format!("{kind}-{ordinal:04}"))
    }
}

#[cfg(test)]
mod tests {
    use super::CentralFixtures;
    use crate::prefix::RunPrefix;

    fn factory() -> CentralFixtures {
        let prefix = RunPrefix::deterministic("central", "0001").expect("a valid label");
        CentralFixtures::new(prefix, 99)
    }

    #[test]
    fn the_same_seed_and_prefix_produce_identical_fixtures() {
        let mut left = factory();
        let mut right = factory();
        for _ in 0..8 {
            let organization = left.organization().expect("a fixture name is accepted");
            assert_eq!(
                organization,
                right.organization().expect("a fixture name is accepted")
            );
            assert_eq!(
                left.workspace(organization.id, "eu-west-1")
                    .expect("a fixture name is accepted"),
                right
                    .workspace(organization.id, "eu-west-1")
                    .expect("a fixture name is accepted")
            );
        }
    }

    #[test]
    fn every_synthetic_name_lives_under_the_run_prefix() {
        let mut fixtures = factory();
        let organization = fixtures.organization().expect("a fixture name is accepted");
        let workspace = fixtures
            .workspace(organization.id, "eu-west-1")
            .expect("a fixture name is accepted");
        let key = fixtures
            .api_key(workspace.id, 1)
            .expect("a fixture name is accepted");
        for name in [&organization.slug, &workspace.slug, &key.public_prefix] {
            assert!(RunPrefix::is_synthetic(name), "{name}");
            assert!(name.starts_with(fixtures.prefix().as_str()), "{name}");
        }
    }

    #[test]
    fn names_and_identifiers_never_repeat() {
        let mut fixtures = factory();
        let mut names = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..64 {
            let organization = fixtures.organization().expect("a fixture name is accepted");
            names.push(organization.slug);
            ids.push(organization.id);
        }
        names.sort_unstable();
        names.dedup();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(names.len(), 64);
        assert_eq!(ids.len(), 64);
    }

    #[test]
    fn each_fixture_advances_the_injected_clock() {
        let mut fixtures = factory();
        let first = fixtures.organization().expect("a fixture name is accepted");
        let second = fixtures.organization().expect("a fixture name is accepted");
        assert!(second.created_at > first.created_at);
    }
}
