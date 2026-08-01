//! The credential-bound central authorization assertion.
//!
//! `central-authz` signs [`signing_input`]; `aex-regional-http` verifies it. This
//! crate owns the canonical bytes and nothing else: the KMS key, the signature
//! algorithm and the verification policy belong to the issuing and verifying
//! services.
//!
//! The lifetime is hard: an assertion may live at most thirty seconds, and the
//! constructor enforces it. A regional edge may reuse one only inside that
//! window and only while its epochs are not older than the local monotonic
//! revocation projection.

use aex_wire::idempotency::{PrincipalKind, PrincipalScope};
use aex_wire::ids::{OrganizationId, SessionId, UserId, WorkspaceId};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{Region, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{Epoch, SchemaVersion};

/// The longest an assertion may live.
pub const MAX_LIFETIME_MS: i64 = 30_000;

/// Who an assertion is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionAudience {
    /// The regional session API.
    RegionalSession,
    /// The regional secret API.
    RegionalSecret,
    /// The regional observation API.
    RegionalObservation,
    /// The regional OTLP admission API.
    RegionalOtlp,
    /// The regional stream service.
    RegionalStream,
}

/// Why an assertion could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssertionError {
    /// The requested lifetime exceeded the hard bound.
    #[error("an assertion may live at most {MAX_LIFETIME_MS} ms")]
    LifetimeTooLong,
    /// `expires_at` was not after `issued_at`.
    #[error("an assertion must expire after it is issued")]
    NotForwardInTime,
}

/// The 30-second credential-bound central assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthorizationAssertion {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Who the credential establishes.
    pub principal: PrincipalScope,
    /// Which principal kind issued it; `user_session` and `account` share a path.
    pub principal_kind: PrincipalKind,
    /// The workspace, when the request is workspace-scoped.
    pub workspace: Option<WorkspaceId>,
    /// The owning organization.
    pub organization: OrganizationId,
    /// The region the assertion is valid in.
    pub region: Region,
    /// The scopes the credential actually carries.
    pub scopes: ScopeSet,
    /// The API key epoch at issue time.
    pub key_epoch: Epoch,
    /// The account epoch at issue time.
    pub account_epoch: Epoch,
    /// The revocation epoch at issue time.
    pub revocation_epoch: Epoch,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// When it stops verifying.
    pub expires_at: Timestamp,
    /// Which service may accept it.
    pub audience: AssertionAudience,
}

impl AuthorizationAssertion {
    /// Checks the hard lifetime bound.
    ///
    /// # Errors
    ///
    /// Returns [`AssertionError`] when the assertion does not expire after it
    /// was issued, or lives longer than [`MAX_LIFETIME_MS`].
    pub fn validate(&self) -> Result<(), AssertionError> {
        let lifetime = self.expires_at.unix_millis() - self.issued_at.unix_millis();
        if lifetime <= 0 {
            return Err(AssertionError::NotForwardInTime);
        }
        if lifetime > MAX_LIFETIME_MS {
            return Err(AssertionError::LifetimeTooLong);
        }
        Ok(())
    }
}

/// The domain-separated signing input.
///
/// The prefix is part of the bytes so a signature over an assertion can never be
/// replayed as a signature over anything else the same key signs.
///
/// # Panics
///
/// Never: the assertion is composed of types that always serialize.
#[must_use]
pub fn signing_input(assertion: &AuthorizationAssertion) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    bytes.extend_from_slice(b"aex:authorization-assertion:v1\x1f");
    bytes.extend_from_slice(
        &aex_wire::to_jcs_bytes(assertion).expect("an assertion always canonicalizes"),
    );
    bytes
}

/// A request for a browser-session assertion over one workspace.
///
/// The clients stream found that `central-authz` could resolve an account token
/// for a workspace but had no browser-session equivalent, which left every
/// regional dashboard panel unimplementable. This is that operation, and it
/// issues the same 30-second assertion through the same path rather than a
/// second credential mechanism.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveSessionForWorkspace {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The browser session being resolved.
    pub browser_session: SessionId,
    /// The signed-in person.
    pub user: UserId,
    /// The workspace the panel is about.
    pub workspace: WorkspaceId,
    /// Which regional service the assertion is for.
    pub audience: AssertionAudience,
}

/// What `resolve_session_for_workspace` answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedSessionAssertion {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The assertion, with principal kind `user_session`.
    pub assertion: AuthorizationAssertion,
}
