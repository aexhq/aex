//! Replay identity.
//!
//! Two headers, two identities, one rule. A mutation that creates a durable
//! operation carries `Aex-Operation-Id`; every other replay-sensitive mutation
//! carries `Idempotency-Key`. Both identities bind the authenticated principal,
//! the canonical route and the canonical intent, so a key reused with a
//! different body is a conflict rather than a second effect.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{ApiKeyId, OperationId, OrganizationId, UserId, WorkspaceId};
use crate::routes::RouteId;
use crate::types::{ValueError, from_str_field};

/// Which replay identity a route requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyKind {
    /// The route is safe to repeat without any replay identity.
    None,
    /// The route requires `Idempotency-Key`.
    IdempotencyKey,
    /// The route requires `Aex-Operation-Id`.
    OperationId,
}

impl IdempotencyKind {
    /// The header this kind is carried in, if any.
    #[must_use]
    pub const fn header(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::IdempotencyKey => Some("Idempotency-Key"),
            Self::OperationId => Some("Aex-Operation-Id"),
        }
    }
}

/// A caller-chosen replay key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(Box<str>);

impl IdempotencyKey {
    /// Largest accepted byte length.
    pub const MAX_BYTES: usize = 256;

    /// Parses a replay key.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the key is empty, longer than
    /// [`IdempotencyKey::MAX_BYTES`], or contains a control character.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "IdempotencyKey";
        if text.is_empty() || text.len() > Self::MAX_BYTES {
            return Err(ValueError::Length {
                kind: KIND,
                min: 1,
                max: Self::MAX_BYTES,
                found: text.len(),
            });
        }
        if let Some(offset) = text.bytes().position(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(ValueError::Character { kind: KIND, offset });
        }
        Ok(Self(text.into()))
    }

    /// The key as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

/// Which kind of principal a credential establishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// An account identity token.
    Account,
    /// A workspace API key, pinned to one region.
    WorkspaceKey,
    /// A browser session, resolved through the same 30-second assertion.
    UserSession,
    /// No credential at all; only the device-flow routes accept this.
    Anonymous,
}

/// The authenticated principal, as replay identity sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrincipalScope {
    /// A person acting through an account token or a browser session.
    Account {
        /// Who.
        user: UserId,
        /// Which organization, when the request selected one.
        organization: Option<OrganizationId>,
    },
    /// A workspace API key.
    WorkspaceKey {
        /// The key metadata record.
        key: ApiKeyId,
        /// The workspace it authorizes.
        workspace: WorkspaceId,
        /// The owning organization.
        organization: OrganizationId,
    },
}

impl PrincipalScope {
    /// The organization the principal acts in, when there is exactly one.
    #[must_use]
    pub const fn organization(&self) -> Option<OrganizationId> {
        match self {
            Self::Account { organization, .. } => *organization,
            Self::WorkspaceKey { organization, .. } => Some(*organization),
        }
    }

    /// The workspace the principal is pinned to, when it is pinned.
    #[must_use]
    pub const fn workspace(&self) -> Option<WorkspaceId> {
        match self {
            Self::Account { .. } => None,
            Self::WorkspaceKey { workspace, .. } => Some(*workspace),
        }
    }

    /// Which kind of principal this is.
    #[must_use]
    pub const fn kind(&self) -> PrincipalKind {
        match self {
            Self::Account { .. } => PrincipalKind::Account,
            Self::WorkspaceKey { .. } => PrincipalKind::WorkspaceKey,
        }
    }
}

/// The canonical digest of what a request actually asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntentDigest([u8; 32]);

impl IntentDigest {
    /// Wraps a raw digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for IntentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The identity of a durable-operation admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIdentity {
    /// Who asked.
    pub principal: PrincipalScope,
    /// Which route.
    pub route: RouteId,
    /// The caller-minted operation id.
    pub operation_id: OperationId,
    /// What was asked for.
    pub intent: IntentDigest,
}

/// The identity of an ordinary replay-sensitive mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayIdentity {
    /// Who asked.
    pub principal: PrincipalScope,
    /// Which route.
    pub route: RouteId,
    /// The caller-chosen key.
    pub key: IdempotencyKey,
    /// What was asked for.
    pub intent: IntentDigest,
}
