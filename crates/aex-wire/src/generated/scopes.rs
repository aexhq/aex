//! GENERATED — DO NOT EDIT.
//!
//! The authorization scope registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:17cc35493241db0815502eb31a2e2f691aa2e1f1f06e1f489c3fa362ff783369`.
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
    /// `sessions:read` — Read sessions, messages, runs and approvals.
    #[serde(rename = "sessions:read")]
    SessionsRead,
    /// `sessions:write` — Create sessions, send messages, and admit session operations.
    #[serde(rename = "sessions:write")]
    SessionsWrite,
    /// `sessions:delete` — Admit the durable session-deletion operation.
    #[serde(rename = "sessions:delete")]
    SessionsDelete,
    /// `files:read` — Read persisted session files and mint their download grants.
    #[serde(rename = "files:read")]
    FilesRead,
    /// `files:write` — Admit the persist operation.
    #[serde(rename = "files:write")]
    FilesWrite,
    /// `files:live` — Read live workspace files, which may wake a retained session.
    #[serde(rename = "files:live")]
    FilesLive,
    /// `resources:read` — Read registered files, skills, tools, instructions and MCP servers.
    #[serde(rename = "resources:read")]
    ResourcesRead,
    /// `resources:write` — Replace and delete registered resources and stage uploads.
    #[serde(rename = "resources:write")]
    ResourcesWrite,
    /// `secrets:read` — Read secret metadata; values are never readable.
    #[serde(rename = "secrets:read")]
    SecretsRead,
    /// `secrets:write` — Set and delete secrets and register provider credentials.
    #[serde(rename = "secrets:write")]
    SecretsWrite,
    /// `secrets:revoke` — Revoke a secret, cancelling current custody.
    #[serde(rename = "secrets:revoke")]
    SecretsRevoke,
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
        ScopeId::OrganizationsRead,
        ScopeId::OrganizationsWrite,
        ScopeId::MembershipsRead,
        ScopeId::MembershipsWrite,
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
        ScopeId::FilesRead,
        ScopeId::FilesWrite,
        ScopeId::FilesLive,
        ScopeId::ResourcesRead,
        ScopeId::ResourcesWrite,
        ScopeId::SecretsRead,
        ScopeId::SecretsWrite,
        ScopeId::SecretsRevoke,
        ScopeId::TelemetryRead,
        ScopeId::TelemetryWrite,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccountRead => "account:read",
            Self::OrganizationsRead => "organizations:read",
            Self::OrganizationsWrite => "organizations:write",
            Self::MembershipsRead => "memberships:read",
            Self::MembershipsWrite => "memberships:write",
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
            Self::FilesRead => "files:read",
            Self::FilesWrite => "files:write",
            Self::FilesLive => "files:live",
            Self::ResourcesRead => "resources:read",
            Self::ResourcesWrite => "resources:write",
            Self::SecretsRead => "secrets:read",
            Self::SecretsWrite => "secrets:write",
            Self::SecretsRevoke => "secrets:revoke",
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
