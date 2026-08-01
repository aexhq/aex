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

    /// Every declared role, in declaration order.
    #[must_use]
    pub fn roles(&self) -> &[RoleGrant] {
        &self.role
    }

    /// Whether the allowlist gives `role` exactly `privilege` on `relation`.
    ///
    /// Deny by default: a relation the role does not name at all holds no
    /// privilege, which is what makes a new money table safe on the day it is
    /// created rather than on the day somebody remembers to revoke it.
    #[must_use]
    pub fn declares(&self, role: &str, relation: &str, privilege: &str) -> bool {
        self.role(role).is_some_and(|role| {
            role.tables.iter().any(|entry| {
                entry.split_once(':').is_some_and(|(named, privileges)| {
                    named.eq_ignore_ascii_case(relation)
                        && privileges
                            .split(',')
                            .any(|declared| declared.eq_ignore_ascii_case(privilege))
                })
            })
        })
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

#[cfg(test)]
mod tests {
    use super::{GrantSet, grants_path};

    #[test]
    fn the_allowlist_denies_by_default() {
        let grants = GrantSet::load(grants_path()).expect("the committed allowlist parses");
        assert!(grants.declares("aex_finance_api", "finance.account_balance", "SELECT"));
        assert!(
            !grants.declares("aex_finance_api", "finance.journal_transaction", "UPDATE"),
            "no role may mutate journal history"
        );
        assert!(
            !grants.declares("aex_finance_api", "finance.provider_cost_fact", "SELECT"),
            "a relation the role does not name carries no privilege at all"
        );
        assert!(
            !grants.declares("aex_authz", "finance.account_balance", "SELECT"),
            "the authorization reader holds no finance grant"
        );
    }

    #[test]
    fn the_cost_reconciler_can_never_write_a_customer_charge() {
        let grants = GrantSet::load(grants_path()).expect("the committed allowlist parses");
        for relation in [
            "finance.journal_transaction",
            "finance.journal_posting",
            "finance.account_balance",
            "finance.billing_account",
        ] {
            for privilege in ["INSERT", "UPDATE", "DELETE"] {
                assert!(
                    !grants.declares("aex_provider_cost", relation, privilege),
                    "aex_provider_cost holds {privilege} on {relation}"
                );
            }
        }
    }
}
