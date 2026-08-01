//! Identities the contract stream does not mint.
//!
//! `aex-wire` owns every identifier that crosses the customer boundary. Three
//! reachability subjects — a download grant, a query cursor and a registry
//! revision — plus the registry's kind vocabulary are reachability authorities
//! that no public route names by id, so they are declared here, once, in the
//! lowest crate of the regional pure stack.
//!
//! They are declared here rather than in `aex-workspace-domain` because
//! [`crate::pin::Pin`] names all four and `aex-workspace-domain` depends on this
//! crate, not the other way round.

use core::fmt;

use aex_wire::ids::Uuid7;

/// Identifies one minted download grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrantId(pub Uuid7);

/// Identifies one open query cursor holding a root pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CursorId(pub Uuid7);

/// A registry pointer's strong monotone concurrency token.
///
/// It is a concurrency token, not a version: no route reads, lists, restores or
/// copies a prior revision's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

impl Revision {
    /// The revision a freshly created pointer carries.
    pub const FIRST: Self = Self(1);

    /// The next revision.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow, which is unreachable for a per-name counter and
    /// is a corrupted authority rather than a customer condition.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("registry revision overflowed"),
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What kind of thing a registry name points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegistryKind {
    /// A workspace file.
    File,
    /// A skill definition.
    Skill,
    /// A tool definition.
    Tool,
    /// A system instruction.
    Instruction,
    /// An MCP server registration.
    McpServer,
}

impl RegistryKind {
    /// Every kind, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::File,
        Self::Skill,
        Self::Tool,
        Self::Instruction,
        Self::McpServer,
    ];

    /// The stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Skill => "skill",
            Self::Tool => "tool",
            Self::Instruction => "instruction",
            Self::McpServer => "mcp_server",
        }
    }

    /// The stable discriminant used in every canonical byte encoding.
    ///
    /// Fixed by hand so adding a variant cannot silently renumber a persisted
    /// `ETag`.
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        match self {
            Self::File => 1,
            Self::Skill => 2,
            Self::Tool => 3,
            Self::Instruction => 4,
            Self::McpServer => 5,
        }
    }

    /// Resolves a wire spelling. There is no alias table.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

impl fmt::Display for RegistryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{RegistryKind, Revision};

    #[test]
    fn registry_kind_round_trips_and_has_stable_discriminants() {
        for kind in RegistryKind::ALL {
            assert_eq!(RegistryKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(RegistryKind::parse("mcpServer"), None);
        let discriminants: Vec<u8> = RegistryKind::ALL
            .into_iter()
            .map(RegistryKind::discriminant)
            .collect();
        assert_eq!(discriminants, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn revision_starts_at_one_and_advances_by_one() {
        assert_eq!(Revision::FIRST, Revision(1));
        assert_eq!(Revision::FIRST.next(), Revision(2));
    }
}
