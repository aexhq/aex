//! Declarative grant allowlist loading and money-boundary validation.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level grants document.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantSet {
    schema_version: u32,
    role: Vec<RoleGrant>,
}

/// One role's exact privileges.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleGrant {
    /// `PostgreSQL` role name.
    pub name: String,
    /// Schemas with usage.
    pub schemas: Vec<String>,
    /// Compact `<schema>.<table>:<privileges>` entries.
    pub tables: Vec<String>,
}

impl GrantSet {
    /// Parses the committed allowlist.
    ///
    /// # Errors
    /// Rejects I/O, TOML and unsupported schema revisions.
    pub fn load(path: PathBuf) -> Result<Self, GrantError> {
        let text = std::fs::read_to_string(path).map_err(GrantError::Io)?;
        let parsed: Self = toml::from_str(&text).map_err(GrantError::Toml)?;
        if parsed.schema_version != 1 {
            return Err(GrantError::Schema(parsed.schema_version));
        }
        parsed.validate_money_boundaries()?;
        Ok(parsed)
    }

    /// Validates non-negotiable least-privilege boundaries.
    ///
    /// # Errors
    /// Rejects journal mutation, schema-history access, and provider-cost access
    /// to any customer money table.
    pub fn validate_money_boundaries(&self) -> Result<(), GrantError> {
        for role in &self.role {
            if role.schemas.is_empty()
                || role
                    .schemas
                    .iter()
                    .any(|schema| !matches!(schema.as_str(), "finance" | "control" | "identity"))
            {
                return Err(GrantError::MoneyBoundary(role.name.clone()));
            }
            for table in &role.tables {
                let upper = table.to_ascii_uppercase();
                if (table.starts_with("finance.journal_transaction:")
                    || table.starts_with("finance.journal_posting:"))
                    && (upper.contains("UPDATE") || upper.contains("DELETE"))
                {
                    return Err(GrantError::MoneyBoundary(role.name.clone()));
                }
                if table.starts_with("schema_admin.") {
                    return Err(GrantError::MoneyBoundary(role.name.clone()));
                }
                if role.name == "aex_provider_cost"
                    && !table.starts_with("finance.provider_cost_fact:")
                {
                    return Err(GrantError::MoneyBoundary(role.name.clone()));
                }
            }
        }
        Ok(())
    }

    /// Sorted declaration-order role names.
    #[must_use]
    pub fn role_names(&self) -> Vec<&str> {
        self.role.iter().map(|role| role.name.as_str()).collect()
    }

    /// Looks up a declared role.
    #[must_use]
    pub fn role(&self, name: &str) -> Option<&RoleGrant> {
        self.role.iter().find(|role| role.name == name)
    }
}

/// Committed grant document.
#[must_use]
pub fn grants_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central/grants.toml")
}

/// Invalid grants document.
#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    /// Filesystem failure.
    #[error("grants I/O failed: {0}")]
    Io(std::io::Error),
    /// TOML decoding failure.
    #[error("grants TOML is invalid: {0}")]
    Toml(toml::de::Error),
    /// Unsupported document version.
    #[error("grants schema {0} is unsupported")]
    Schema(u32),
    /// A role crossed a non-negotiable money boundary.
    #[error("role `{0}` crosses a money boundary")]
    MoneyBoundary(String),
}
