//! Deterministic regional-plane fixtures.
//!
//! Every value is derived from the injected clock, the seeded identifier factory
//! and the run prefix, so two runs with the same seed produce identical inputs.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::TestClock;
use crate::ids::IdFactory;
use crate::prefix::{PrefixError, RunPrefix};

/// A session as the session authority would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFixture {
    /// Session identifier.
    pub id: Uuid,
    /// Owning workspace.
    pub workspace_id: Uuid,
    /// Synthetic name inside the run namespace.
    pub name: String,
    /// Creation instant read from the injected clock.
    pub created_at: OffsetDateTime,
}

/// A content descriptor as the content authority would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFixture {
    /// Descriptor identifier.
    pub id: Uuid,
    /// Content-addressed digest, rendered the way the descriptor stores it.
    pub digest: String,
    /// Object size in bytes.
    pub byte_len: u64,
    /// Creation instant read from the injected clock.
    pub created_at: OffsetDateTime,
}

/// A durable operation as the work authority would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFixture {
    /// Operation identifier.
    pub id: Uuid,
    /// The session the operation belongs to.
    pub session_id: Uuid,
    /// Declared operation kind.
    pub kind: String,
    /// Fence recorded with the claim.
    pub fence: u64,
    /// When the operation becomes due.
    pub due_at: OffsetDateTime,
}

/// Builds regional fixtures from one clock, one seed and one run prefix.
#[derive(Debug, Clone)]
pub struct RegionalFixtures {
    clock: TestClock,
    ids: IdFactory,
    prefix: RunPrefix,
    issued: u64,
}

impl RegionalFixtures {
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

    /// A fresh session in `workspace_id`.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated name.
    pub fn session(&mut self, workspace_id: Uuid) -> Result<SessionFixture, PrefixError> {
        let name = self.next_name("session")?;
        Ok(SessionFixture {
            id: self.ids.next_id(),
            workspace_id,
            name,
            created_at: self.clock.tick(),
        })
    }

    /// A fresh content descriptor of `byte_len` bytes.
    #[must_use]
    pub fn content(&mut self, byte_len: u64) -> ContentFixture {
        let id = self.ids.next_id();
        ContentFixture {
            id,
            digest: format!("blake3:{}", id.simple()),
            byte_len,
            created_at: self.clock.tick(),
        }
    }

    /// A fresh durable operation of `kind` for `session_id`.
    #[must_use]
    pub fn operation(&mut self, session_id: Uuid, kind: &str) -> OperationFixture {
        let fence = self.issued.saturating_add(1);
        self.issued = fence;
        OperationFixture {
            id: self.ids.next_id(),
            session_id,
            kind: kind.to_owned(),
            fence,
            due_at: self.clock.tick(),
        }
    }

    fn next_name(&mut self, kind: &str) -> Result<String, PrefixError> {
        let ordinal = self.issued;
        self.issued = self.issued.saturating_add(1);
        self.prefix.resource(&format!("{kind}-{ordinal:04}"))
    }
}

#[cfg(test)]
mod tests {
    use super::RegionalFixtures;
    use crate::prefix::RunPrefix;
    use uuid::Uuid;

    fn factory() -> RegionalFixtures {
        let prefix = RunPrefix::deterministic("regional", "0001").expect("a valid label");
        RegionalFixtures::new(prefix, 11)
    }

    #[test]
    fn the_same_seed_and_prefix_produce_identical_fixtures() {
        let workspace = Uuid::from_u128(7);
        let mut left = factory();
        let mut right = factory();
        for _ in 0..8 {
            assert_eq!(
                left.session(workspace).expect("a fixture name is accepted"),
                right
                    .session(workspace)
                    .expect("a fixture name is accepted")
            );
            assert_eq!(left.content(4_096), right.content(4_096));
            assert_eq!(
                left.operation(workspace, "export"),
                right.operation(workspace, "export")
            );
        }
    }

    #[test]
    fn every_synthetic_name_lives_under_the_run_prefix() {
        let mut fixtures = factory();
        let session = fixtures
            .session(Uuid::from_u128(1))
            .expect("a fixture name is accepted");
        assert!(RunPrefix::is_synthetic(&session.name), "{}", session.name);
        assert!(session.name.starts_with(fixtures.prefix().as_str()));
    }

    #[test]
    fn content_digests_are_derived_from_the_descriptor_identity() {
        let mut fixtures = factory();
        let content = fixtures.content(64);
        assert_eq!(content.digest, format!("blake3:{}", content.id.simple()));
        assert_eq!(content.byte_len, 64);
    }

    #[test]
    fn every_operation_fence_is_strictly_increasing() {
        let mut fixtures = factory();
        let session = Uuid::from_u128(3);
        let mut previous = 0;
        for _ in 0..32 {
            let operation = fixtures.operation(session, "purge");
            assert!(
                operation.fence > previous,
                "{} must exceed {previous}",
                operation.fence
            );
            previous = operation.fence;
        }
    }
}
