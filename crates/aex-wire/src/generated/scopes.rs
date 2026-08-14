//! GENERATED — DO NOT EDIT.
//!
//! The authorization scope registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:ec8637e9442587d0020caccfed0ab6fccef5a6d61dae1c162fe2d78e2af0dec8`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use serde::{Deserialize, Serialize};

/// Every scope a workspace API key or dashboard session can carry. The set is derived from the
/// route table: a scope this enum lacks is a route change, not a registry change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeId {
    /// `account:read` — Read bootstrap and personal-account state.
    #[serde(rename = "account:read")]
    AccountRead,
    /// `account:write` — Close the caller's own browser session.
    #[serde(rename = "account:write")]
    AccountWrite,
    /// `api_keys:read` — List fixed-workspace API key metadata.
    #[serde(rename = "api_keys:read")]
    ApiKeysRead,
    /// `api_keys:write` — Mint and revoke fixed-workspace API keys.
    #[serde(rename = "api_keys:write")]
    ApiKeysWrite,
    /// `billing:read` — Read prepaid balance, card metadata, transactions and rated usage.
    #[serde(rename = "billing:read")]
    BillingRead,
    /// `billing:write` — Set up or remove cards and create manual top-up checkouts.
    #[serde(rename = "billing:write")]
    BillingWrite,
    /// `sessions:read` — Read sessions, messages and telemetry.
    #[serde(rename = "sessions:read")]
    SessionsRead,
    /// `sessions:write` — Create sessions, send messages, cancel and terminate.
    #[serde(rename = "sessions:write")]
    SessionsWrite,
    /// `sessions:delete` — Admit irreversible session deletion.
    #[serde(rename = "sessions:delete")]
    SessionsDelete,
    /// `resources:read` — Read current workspace files and mint downloads.
    #[serde(rename = "resources:read")]
    ResourcesRead,
    /// `resources:write` — Overwrite/delete current workspace files and upload bytes.
    #[serde(rename = "resources:write")]
    ResourcesWrite,
}

impl ScopeId {
    /// Every scope, in registry order.
    pub const ALL: &'static [ScopeId] = &[
        ScopeId::AccountRead,
        ScopeId::AccountWrite,
        ScopeId::ApiKeysRead,
        ScopeId::ApiKeysWrite,
        ScopeId::BillingRead,
        ScopeId::BillingWrite,
        ScopeId::SessionsRead,
        ScopeId::SessionsWrite,
        ScopeId::SessionsDelete,
        ScopeId::ResourcesRead,
        ScopeId::ResourcesWrite,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccountRead => "account:read",
            Self::AccountWrite => "account:write",
            Self::ApiKeysRead => "api_keys:read",
            Self::ApiKeysWrite => "api_keys:write",
            Self::BillingRead => "billing:read",
            Self::BillingWrite => "billing:write",
            Self::SessionsRead => "sessions:read",
            Self::SessionsWrite => "sessions:write",
            Self::SessionsDelete => "sessions:delete",
            Self::ResourcesRead => "resources:read",
            Self::ResourcesWrite => "resources:write",
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
