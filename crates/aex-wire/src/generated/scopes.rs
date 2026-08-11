//! GENERATED — DO NOT EDIT.
//!
//! The authorization scope registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:90fdf2649aa0c7151830e06eb94993aeafa67faf7c867042dbff4638af9aafba`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use serde::{Deserialize, Serialize};

/// Every scope a workspace API key or account token can carry. The set is derived from the route
/// table: a scope this enum lacks is a route change, not a registry change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeId {
    /// `account:read` — Read the caller's account and operational state.
    #[serde(rename = "account:read")]
    AccountRead,
    /// `account:write` — Decide the caller's own device authorizations and close their own browser
    /// session.
    #[serde(rename = "account:write")]
    AccountWrite,
    /// `organizations:read` — List and read organizations the caller belongs to.
    #[serde(rename = "organizations:read")]
    OrganizationsRead,
    /// `organizations:write` — Create organizations.
    #[serde(rename = "organizations:write")]
    OrganizationsWrite,
    /// `memberships:read` — List memberships of an organization.
    #[serde(rename = "memberships:read")]
    MembershipsRead,
    /// `memberships:write` — Invite people to an organization.
    #[serde(rename = "memberships:write")]
    MembershipsWrite,
    /// `memberships:accept` — Redeem invitations addressed to the caller's own verified email.
    #[serde(rename = "memberships:accept")]
    MembershipsAccept,
    /// `workspaces:read` — List and read workspaces.
    #[serde(rename = "workspaces:read")]
    WorkspacesRead,
    /// `workspaces:write` — Create workspaces.
    #[serde(rename = "workspaces:write")]
    WorkspacesWrite,
    /// `workspaces:delete` — Admit the global workspace-deletion operation.
    #[serde(rename = "workspaces:delete")]
    WorkspacesDelete,
    /// `api_keys:read` — List workspace API key metadata.
    #[serde(rename = "api_keys:read")]
    ApiKeysRead,
    /// `api_keys:write` — Mint and revoke workspace API keys.
    #[serde(rename = "api_keys:write")]
    ApiKeysWrite,
    /// `billing:read` — Read balances, statements and usage.
    #[serde(rename = "billing:read")]
    BillingRead,
    /// `billing:write` — Create checkouts, portal sessions and top-up policy.
    #[serde(rename = "billing:write")]
    BillingWrite,
    /// `operations:read` — Read durable operation records.
    #[serde(rename = "operations:read")]
    OperationsRead,
    /// `operations:write` — Request cancellation of a durable operation.
    #[serde(rename = "operations:write")]
    OperationsWrite,
    /// `workspace:read` — Read the current workspace and its effective limits.
    #[serde(rename = "workspace:read")]
    WorkspaceRead,
    /// `sessions:read` — Read sessions, sealed messages and approvals.
    #[serde(rename = "sessions:read")]
    SessionsRead,
    /// `sessions:write` — Create sessions, send messages, and cancel, suspend, resume or terminate
    /// a session.
    #[serde(rename = "sessions:write")]
    SessionsWrite,
    /// `sessions:delete` — Admit irreversible session deletion.
    #[serde(rename = "sessions:delete")]
    SessionsDelete,
    /// `files:live` — Access live session files, automatically resuming the same suspended
    /// generation.
    #[serde(rename = "files:live")]
    FilesLive,
    /// `resources:read` — Read registered opaque workspace files and mint their download grants.
    #[serde(rename = "resources:read")]
    ResourcesRead,
    /// `resources:write` — Replace and delete registered opaque workspace files and stage uploads.
    #[serde(rename = "resources:write")]
    ResourcesWrite,
    /// `provider_credentials:read` — Read dedicated BYOK provider-credential binding metadata.
    #[serde(rename = "provider_credentials:read")]
    ProviderCredentialsRead,
    /// `provider_credentials:write` — Register and revoke dedicated BYOK provider credentials.
    #[serde(rename = "provider_credentials:write")]
    ProviderCredentialsWrite,
    /// `telemetry:read` — Query observations, gaps and exports.
    #[serde(rename = "telemetry:read")]
    TelemetryRead,
    /// `telemetry:write` — Admit customer OTLP batches.
    #[serde(rename = "telemetry:write")]
    TelemetryWrite,
}

impl ScopeId {
    /// Every scope, in registry order.
    pub const ALL: &'static [ScopeId] = &[
        ScopeId::AccountRead,
        ScopeId::AccountWrite,
        ScopeId::OrganizationsRead,
        ScopeId::OrganizationsWrite,
        ScopeId::MembershipsRead,
        ScopeId::MembershipsWrite,
        ScopeId::MembershipsAccept,
        ScopeId::WorkspacesRead,
        ScopeId::WorkspacesWrite,
        ScopeId::WorkspacesDelete,
        ScopeId::ApiKeysRead,
        ScopeId::ApiKeysWrite,
        ScopeId::BillingRead,
        ScopeId::BillingWrite,
        ScopeId::OperationsRead,
        ScopeId::OperationsWrite,
        ScopeId::WorkspaceRead,
        ScopeId::SessionsRead,
        ScopeId::SessionsWrite,
        ScopeId::SessionsDelete,
        ScopeId::FilesLive,
        ScopeId::ResourcesRead,
        ScopeId::ResourcesWrite,
        ScopeId::ProviderCredentialsRead,
        ScopeId::ProviderCredentialsWrite,
        ScopeId::TelemetryRead,
        ScopeId::TelemetryWrite,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccountRead => "account:read",
            Self::AccountWrite => "account:write",
            Self::OrganizationsRead => "organizations:read",
            Self::OrganizationsWrite => "organizations:write",
            Self::MembershipsRead => "memberships:read",
            Self::MembershipsWrite => "memberships:write",
            Self::MembershipsAccept => "memberships:accept",
            Self::WorkspacesRead => "workspaces:read",
            Self::WorkspacesWrite => "workspaces:write",
            Self::WorkspacesDelete => "workspaces:delete",
            Self::ApiKeysRead => "api_keys:read",
            Self::ApiKeysWrite => "api_keys:write",
            Self::BillingRead => "billing:read",
            Self::BillingWrite => "billing:write",
            Self::OperationsRead => "operations:read",
            Self::OperationsWrite => "operations:write",
            Self::WorkspaceRead => "workspace:read",
            Self::SessionsRead => "sessions:read",
            Self::SessionsWrite => "sessions:write",
            Self::SessionsDelete => "sessions:delete",
            Self::FilesLive => "files:live",
            Self::ResourcesRead => "resources:read",
            Self::ResourcesWrite => "resources:write",
            Self::ProviderCredentialsRead => "provider_credentials:read",
            Self::ProviderCredentialsWrite => "provider_credentials:write",
            Self::TelemetryRead => "telemetry:read",
            Self::TelemetryWrite => "telemetry:write",
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
