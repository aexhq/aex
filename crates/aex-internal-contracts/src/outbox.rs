//! The terminal outbox event and the identities it carries.
//!
//! The terminal barrier writes exactly one durable notification, and two
//! independently deployed processes read it: `regional-stream` delivers it, and
//! the observation materializer projects it. That makes it an envelope crossing
//! a process boundary rather than a value internal to the session domain, so it
//! lives here and `aex-session-domain` imports it.
//!
//! [`RunStatus`], [`SessionRevision`] and [`UsageClosureId`] moved with it: an
//! envelope whose members live in a crate the readers do not depend on is not
//! decodable, which is the whole point of moving it.
//!
//! [`RunStatus`] is deliberately internal. Public run resources were removed in
//! the session-centric MVP, while this private status still closes execution,
//! usage and observation authorities. A boundary test pins its stored spelling.

use core::fmt;

use aex_wire::Uuid7;
use aex_wire::ids::SessionId;

use crate::RunId;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// Where a run ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Admitted, not started.
    Queued,
    /// Executing.
    Running,
    /// Finished normally.
    Succeeded,
    /// Finished with a domain error.
    Failed,
    /// Ran past its deadline.
    TimedOut,
    /// Cancelled by an operation.
    Cancelled,
    /// Fenced by the platform.
    Interrupted,
}

impl RunStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 7] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::TimedOut,
        Self::Cancelled,
        Self::Interrupted,
    ];

    /// Whether no transition leaves this status.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled | Self::Interrupted
        )
    }
}

/// The session head's optimistic concurrency token.
///
/// A distinct newtype rather than a bare `u64` because comparing a session
/// revision against an agent revision is the kind of mistake that produces a
/// silently wrong fence rather than a loud failure. It renders as a canonical
/// decimal string so a reader never has to think about integer width.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionRevision(#[serde(with = "decimal_string")] pub u64);

impl SessionRevision {
    /// The value a freshly created record carries.
    pub const INITIAL: Self = Self(1);

    /// The next value.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow, which is a corrupted authority rather than a
    /// customer condition.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("SessionRevision overflowed"),
        }
    }
}

impl fmt::Display for SessionRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The immutable identity of one run's usage closure.
///
/// Minted in the terminal barrier and never rewritten, so a replayed settlement
/// resolves to the same closure rather than opening a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UsageClosureId(pub Uuid7);

/// The native outbox event a terminal barrier emits.
///
/// It is the one durable notification the barrier writes, inside the same
/// transaction as the settlement, so nothing outside that transaction can
/// prevent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OutboxEvent {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The owning session.
    pub session: SessionId,
    /// The run that settled.
    pub run: RunId,
    /// The status it settled at.
    pub status: RunStatus,
    /// The session revision after the barrier.
    pub session_revision: SessionRevision,
    /// The usage closure the run is billed under.
    pub usage_closure: UsageClosureId,
    /// When the barrier committed.
    pub at: Timestamp,
}

/// Canonical decimal-string serde for a `u64` counter.
mod decimal_string {
    use serde::de::Error as _;
    use serde::{Deserializer, Serializer};

    /// Renders the counter as a canonical decimal string.
    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `with` module contract fixes this signature"
    )]
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Parses a canonical decimal string, rejecting a leading zero or a sign.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = <&str as serde::Deserialize>::deserialize(deserializer)?;
        if text.is_empty()
            || !text.bytes().all(|byte| byte.is_ascii_digit())
            || (text.len() > 1 && text.starts_with('0'))
        {
            return Err(D::Error::custom(
                "a revision is a canonical non-negative decimal string",
            ));
        }
        text.parse().map_err(D::Error::custom)
    }
}
