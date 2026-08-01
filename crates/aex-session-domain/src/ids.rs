//! The monotone counters, epochs and derived identities the session authority
//! keeps.
//!
//! Every one of them is a distinct newtype rather than a bare `u64`, because
//! comparing a session revision against an agent revision is the kind of mistake
//! that produces a silently wrong fence rather than a loud failure.

use core::fmt;

use aex_wire::ids::Uuid7;

/// Declares one monotone `u64` counter.
macro_rules! counter {
    ($(#[$meta:meta])* $name:ident, $initial:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl $name {
            /// The value a freshly created record carries.
            pub const INITIAL: Self = Self($initial);

            /// The next value.
            ///
            /// # Panics
            ///
            /// Panics on `u64` overflow, which is a corrupted authority rather
            /// than a customer condition.
            #[must_use]
            pub const fn next(self) -> Self {
                match self.0.checked_add(1) {
                    Some(value) => Self(value),
                    None => panic!(concat!(stringify!($name), " overflowed")),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

// The session revision travels on the terminal outbox event, so it is owned by
// the contract crate both readers of that event depend on. Re-exported here so
// every call site keeps its `aex_session_domain` path.
pub use aex_internal_contracts::outbox::SessionRevision;

counter!(
    /// One agent control record's optimistic concurrency token.
    AgentRevision,
    1
);
counter!(
    /// A position in one agent's journal. Zero means "no entry yet".
    JournalSeq,
    0
);
counter!(
    /// The claim fence an agent's owner must present on every write.
    AgentFence,
    0
);
counter!(
    /// Advances by exactly one per effective session-wide cancellation.
    CancellationEpoch,
    0
);
counter!(
    /// The organization projection's revision.
    AccountRevision,
    0
);
counter!(
    /// The workspace authorization epoch a command was built against.
    AuthorizationEpoch,
    0
);
counter!(
    /// How many times the session's durable root has advanced.
    PersistRevision,
    0
);

/// The content-derived identity of one journal entry.
///
/// A duplicate identity collapses to the first occurrence, which is what makes a
/// retried append idempotent without a second write path.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryIdentity([u8; 32]);

impl EntryIdentity {
    /// Wraps a precomputed identity.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw identity.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for EntryIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EntryIdentity(")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        formatter.write_str(")")
    }
}

// The usage-closure identity travels on the terminal outbox event, so it too is
// owned by the contract crate and re-exported here.
pub use aex_internal_contracts::outbox::UsageClosureId;

/// The identity of one spend reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReservationId(pub Uuid7);

/// The identity of one open effect an agent has prepared but not settled.
///
/// `aex-session-domain` owns only the identity and the balance rule; the
/// prepare/open/complete receipt protocol is `aex-brain-domain`'s (D-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId(pub Uuid7);

#[cfg(test)]
mod tests {
    use super::{CancellationEpoch, EntryIdentity, JournalSeq, SessionRevision};

    #[test]
    fn counters_start_where_they_are_declared_and_advance_by_one() {
        assert_eq!(SessionRevision::INITIAL, SessionRevision(1));
        assert_eq!(JournalSeq::INITIAL, JournalSeq(0));
        assert_eq!(CancellationEpoch::INITIAL.next(), CancellationEpoch(1));
        assert_eq!(SessionRevision(7).next(), SessionRevision(8));
    }

    #[test]
    fn an_entry_identity_renders_as_hex() {
        let identity = EntryIdentity::from_bytes([0xab; 32]);
        assert!(format!("{identity:?}").contains("abab"));
        assert_eq!(identity.as_bytes(), &[0xab; 32]);
    }
}
