//! The declarative privilege allowlist, and the exact SQL it renders to.
//!
//! `migrations/central/grants.toml` is the only place a privilege in the central
//! database is written down. A migration creates objects; this document says who
//! may touch them. [`GrantSet::render`] turns the document into the ordered
//! statement list that `central-schema-admin grants --apply` executes and that
//! the container-backed suite in `crates/aex-control-aurora/tests/migrations.rs`
//! applies before probing the denial matrix — one renderer, so the matrix the
//! tests prove is the matrix production gets.
//!
//! # Why loading validates so much
//!
//! Every reference in the document is checked at load: a role's table must sit
//! in a schema that role holds `USAGE` on, a function it lists must be declared,
//! privileges come from a closed four-value set, and four architectural
//! boundaries are refused outright ([`GrantSet::validate`]). So every identifier
//! that reaches [`GrantSet::render`] has already been proven to match
//! `^[a-z][a-z0-9_]*$`, and the rendered SQL interpolates identifiers that were
//! checked rather than identifiers a reviewer was trusted to have read.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;

const EMBEDDED_GRANTS: &str = include_str!("../../../migrations/central/grants.toml");

/// The only privileges this document uses.
///
/// `TRUNCATE`, `REFERENCES` and `TRIGGER` are absent deliberately: no central
/// deployable needs one, and admitting a spelling nobody uses is how an
/// allowlist stops being an allowlist.
pub const PRIVILEGES: [&str; 4] = ["SELECT", "INSERT", "UPDATE", "DELETE"];

/// Schemas an application role may ever hold `USAGE` on.
///
/// `schema_admin` holds the `SQLx` migration history and the backfill cursor. It
/// is deliberately absent: a role that reads the history reads which migrations
/// a plane is missing.
const ROLE_SCHEMAS: [&str; 3] = ["identity", "control", "finance"];

/// Tables no role may ever write, in any form.
const NEVER_WRITABLE: [&str; 1] = ["control.authorization_epoch"];

/// Tables that admit `INSERT` and never `UPDATE` or `DELETE`.
///
/// The audit log and the journal are corrected by appending — a second event, a
/// reversal — never by editing what was recorded.
const APPEND_ONLY: [&str; 3] = [
    "control.audit_event",
    "finance.journal_transaction",
    "finance.journal_posting",
];

/// Roles whose every privilege must be `SELECT`.
///
/// `central-authz` runs its write probe at start-up and requires it to *fail*. A
/// document in which it could succeed is refused here, so the misconfiguration
/// cannot reach the plane where the probe would have discovered it.
const READ_ONLY_ROLES: [&str; 1] = ["aex_authz"];

/// Top-level grants document.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantSet {
    schema_version: u32,
    database: DatabaseGrant,
    schema: Vec<SchemaGrant>,
    function: Vec<FunctionGrant>,
    role: Vec<RoleGrant>,
}

/// The database-wide denial.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseGrant {
    /// Whether `PUBLIC` loses every database privilege, `CONNECT` included.
    pub revoke_public: bool,
}

/// One schema's baseline denial.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaGrant {
    /// Schema name.
    pub name: String,
    /// Whether `PUBLIC` loses every privilege on it.
    pub revoke_public: bool,
}

/// One function's `EXECUTE` policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionGrant {
    /// Fully qualified `schema.name(argument types)`.
    pub name: String,
    /// Whether `PUBLIC` loses `EXECUTE`, which it holds by default.
    pub revoke_public: bool,
    /// Whether naming this function in any role's `functions` is an error.
    #[serde(default)]
    pub never_granted: bool,
}

/// One role's exact privileges.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleGrant {
    /// `PostgreSQL` role name.
    pub name: String,
    /// Whether the role may open a session on the database.
    pub connect: bool,
    /// Schemas with `USAGE`.
    pub schemas: Vec<String>,
    /// Compact `<schema>.<table>:<privileges>` entries.
    pub tables: Vec<String>,
    /// Declared functions this role may `EXECUTE`.
    #[serde(default)]
    pub functions: Vec<String>,
}

impl RoleGrant {
    /// Splits one `<schema>.<table>:<privileges>` entry.
    fn parse_table<'entry>(
        &self,
        entry: &'entry str,
    ) -> Result<(&'entry str, Vec<&'static str>), GrantError> {
        let invalid = || GrantError::Table {
            role: self.name.clone(),
            entry: entry.to_owned(),
        };
        let (table, privileges) = entry.split_once(':').ok_or_else(invalid)?;
        let (schema, relation) = table.split_once('.').ok_or_else(invalid)?;
        if !is_identifier(schema) || !is_identifier(relation) {
            return Err(invalid());
        }
        if !self.schemas.iter().any(|held| held == schema) {
            return Err(invalid());
        }
        let mut parsed: Vec<&'static str> = Vec::new();
        for privilege in privileges.split(',') {
            let known = PRIVILEGES
                .iter()
                .find(|known| **known == privilege)
                .ok_or_else(invalid)?;
            if parsed.contains(known) {
                return Err(invalid());
            }
            parsed.push(known);
        }
        Ok((table, parsed))
    }
}

impl GrantSet {
    /// Parses and validates the allowlist compiled into the executable.
    ///
    /// # Errors
    /// Rejects the same malformed or architecturally invalid document as
    /// [`GrantSet::load`], without reading a runtime filesystem path.
    pub fn embedded() -> Result<Self, GrantError> {
        Self::parse(EMBEDDED_GRANTS)
    }

    /// Parses and validates the committed allowlist.
    ///
    /// # Errors
    /// Rejects I/O, TOML, unsupported document versions, malformed or dangling
    /// entries, and every boundary in [`GrantSet::validate`].
    pub fn load(path: PathBuf) -> Result<Self, GrantError> {
        let text = std::fs::read_to_string(path).map_err(GrantError::Io)?;
        Self::parse(&text)
    }

    /// Parses and validates a document already in memory.
    ///
    /// # Errors
    /// As [`GrantSet::load`], without the I/O arm.
    pub fn parse(text: &str) -> Result<Self, GrantError> {
        let parsed: Self = toml::from_str(text).map_err(GrantError::Toml)?;
        if parsed.schema_version != 2 {
            return Err(GrantError::Version(parsed.schema_version));
        }
        parsed.validate()?;
        Ok(parsed)
    }

    /// Validates syntax, every cross-reference, and four boundaries.
    ///
    /// The boundaries are not style. Each is a claim the architecture makes
    /// about what this database can be asked to do at all.
    ///
    /// 1. `control.authorization_epoch` is never writable, so a revoked
    ///    assertion cannot be un-revoked by a decrement.
    /// 2. The audit log and the journal are append-only, so history is corrected
    ///    by appending rather than by editing.
    /// 3. `central-authz` holds nothing but `SELECT`, so its start-up write
    ///    probe cannot pass.
    /// 4. `aex_provider_cost` reaches no customer money table, and no role
    ///    reaches `schema_admin`.
    ///
    /// # Errors
    /// Returns the first rule violated, naming the role and the entry.
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> Result<(), GrantError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for schema in &self.schema {
            if !is_identifier(&schema.name) || !seen.insert(&schema.name) {
                return Err(GrantError::SchemaEntry(schema.name.clone()));
            }
        }
        seen.clear();
        for function in &self.function {
            if !is_function_signature(&function.name) || !seen.insert(&function.name) {
                return Err(GrantError::Function(function.name.clone()));
            }
        }
        seen.clear();
        for role in &self.role {
            if !is_role_name(&role.name) || !seen.insert(&role.name) {
                return Err(GrantError::Role(role.name.clone()));
            }
            let boundary = |reason| GrantError::Boundary {
                role: role.name.clone(),
                reason,
            };
            if role.schemas.is_empty()
                || role
                    .schemas
                    .iter()
                    .any(|schema| !ROLE_SCHEMAS.contains(&schema.as_str()))
            {
                return Err(boundary("holds a schema outside identity/control/finance"));
            }
            let read_only = READ_ONLY_ROLES.contains(&role.name.as_str());
            for entry in &role.tables {
                let (table, privileges) = role.parse_table(entry)?;
                let writes = privileges.iter().any(|privilege| *privilege != "SELECT");
                let mutates = privileges
                    .iter()
                    .any(|privilege| *privilege == "UPDATE" || *privilege == "DELETE");
                if read_only && writes {
                    return Err(boundary("is read-only and holds a write"));
                }
                if NEVER_WRITABLE.contains(&table) && writes {
                    return Err(boundary("writes a table no role may write"));
                }
                if APPEND_ONLY.contains(&table) && mutates {
                    return Err(boundary("mutates an append-only table"));
                }
                if role.name == "aex_provider_cost" && table != "finance.provider_cost_fact" {
                    return Err(boundary("reaches a table outside the provider-cost fact"));
                }
            }
            for signature in &role.functions {
                let declared = self
                    .function
                    .iter()
                    .find(|function| &function.name == signature)
                    .ok_or_else(|| GrantError::Function(signature.clone()))?;
                if declared.never_granted {
                    return Err(boundary("executes a function declared never_granted"));
                }
                let schema = signature.split('.').next().unwrap_or_default();
                if !role.schemas.iter().any(|held| held == schema) {
                    return Err(boundary("executes a function in a schema it cannot use"));
                }
            }
        }
        Ok(())
    }

    /// Renders the exact statements, in the order they must run.
    ///
    /// Revocations come first and grants second, so a `revoke_public` can never
    /// take back a privilege the same run just handed out. `database` is the one
    /// value that does not come from the committed document, so it is the one
    /// value checked here.
    ///
    /// # Errors
    /// Returns [`GrantError::Database`] for a database name that is not a bare
    /// lower-case identifier, and [`GrantError::Table`] for an entry that
    /// [`GrantSet::validate`] would have refused.
    pub fn render(&self, database: &str) -> Result<Vec<String>, GrantError> {
        if !is_identifier(database) || database.len() > 63 {
            return Err(GrantError::Database(database.to_owned()));
        }
        let mut statements = Vec::new();
        if self.database.revoke_public {
            statements.push(format!("REVOKE ALL ON DATABASE {database} FROM PUBLIC"));
        }
        for schema in &self.schema {
            if schema.revoke_public {
                statements.push(format!("REVOKE ALL ON SCHEMA {} FROM PUBLIC", schema.name));
            }
        }
        for function in &self.function {
            if function.revoke_public {
                statements.push(format!(
                    "REVOKE EXECUTE ON FUNCTION {} FROM PUBLIC",
                    function.name
                ));
            }
        }
        for role in &self.role {
            let name = &role.name;
            if role.connect {
                statements.push(format!("GRANT CONNECT ON DATABASE {database} TO {name}"));
            }
            for schema in &role.schemas {
                statements.push(format!("GRANT USAGE ON SCHEMA {schema} TO {name}"));
            }
            for entry in &role.tables {
                let (table, privileges) = role.parse_table(entry)?;
                statements.push(format!(
                    "GRANT {} ON {table} TO {name}",
                    privileges.join(", ")
                ));
            }
            for signature in &role.functions {
                statements.push(format!("GRANT EXECUTE ON FUNCTION {signature} TO {name}"));
            }
        }
        Ok(statements)
    }

    /// Declaration-order role names.
    #[must_use]
    pub fn role_names(&self) -> Vec<&str> {
        self.role.iter().map(|role| role.name.as_str()).collect()
    }

    /// Database-wide `PUBLIC` policy.
    #[must_use]
    pub const fn database_policy(&self) -> &DatabaseGrant {
        &self.database
    }

    /// Every declared schema policy, in declaration order.
    #[must_use]
    pub fn schemas(&self) -> &[SchemaGrant] {
        &self.schema
    }

    /// Every declared function policy, in declaration order.
    #[must_use]
    pub fn functions(&self) -> &[FunctionGrant] {
        &self.function
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

    /// Declaration-order function signatures.
    #[must_use]
    pub fn function_names(&self) -> Vec<&str> {
        self.function
            .iter()
            .map(|function| function.name.as_str())
            .collect()
    }
}

/// Whether `text` is a bare lower-case SQL identifier.
fn is_identifier(text: &str) -> bool {
    text.starts_with(|first: char| first.is_ascii_lowercase())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Whether `text` is `aex_`-prefixed and otherwise a bare identifier.
fn is_role_name(text: &str) -> bool {
    text.starts_with("aex_") && is_identifier(text)
}

/// Whether `text` is `schema.name(type[, type])`.
fn is_function_signature(text: &str) -> bool {
    let Some((qualified, rest)) = text.split_once('(') else {
        return false;
    };
    let Some(arguments) = rest.strip_suffix(')') else {
        return false;
    };
    let Some((schema, name)) = qualified.split_once('.') else {
        return false;
    };
    is_identifier(schema)
        && is_identifier(name)
        && (arguments.is_empty() || arguments.split(", ").all(is_identifier))
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
    Version(u32),
    /// A schema entry is malformed or repeated.
    #[error("schema `{0}` is malformed or declared twice")]
    SchemaEntry(String),
    /// A function entry is malformed, repeated, or named but never declared.
    #[error("function `{0}` is malformed, declared twice, or undeclared")]
    Function(String),
    /// A role entry is malformed or repeated.
    #[error("role `{0}` is malformed or declared twice")]
    Role(String),
    /// A `<schema>.<table>:<privileges>` entry is malformed or out of scope.
    #[error("role `{role}` declares invalid table entry `{entry}`")]
    Table {
        /// The role that declared it.
        role: String,
        /// The offending entry.
        entry: String,
    },
    /// A role crossed a non-negotiable boundary.
    #[error("role `{role}` {reason}")]
    Boundary {
        /// The role that crossed it.
        role: String,
        /// Which boundary, in the rule's own words.
        reason: &'static str,
    },
    /// The database name to render is not a bare identifier.
    #[error("`{0}` is not a bare lower-case database name")]
    Database(String),
}

#[cfg(test)]
mod tests {
    use super::{GrantError, GrantSet};

    fn grants_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../migrations/central/grants.toml")
    }

    /// The committed document with one substring replaced.
    fn edited(from: &str, to: &str) -> GrantError {
        let text = std::fs::read_to_string(grants_path()).expect("the committed document reads");
        assert!(text.contains(from), "the anchor `{from}` still exists");
        GrantSet::parse(&text.replace(from, to)).expect_err("the edited document is refused")
    }

    #[test]
    fn the_embedded_allowlist_is_the_authored_allowlist() {
        let authored = GrantSet::load(grants_path()).expect("the authored allowlist parses");
        let embedded = GrantSet::embedded().expect("the embedded allowlist parses");
        assert_eq!(
            embedded.render("aex").expect("embedded grants render"),
            authored.render("aex").expect("authored grants render")
        );
    }

    #[test]
    fn every_application_role_gets_connect_and_public_never_does() {
        let grants = GrantSet::load(grants_path()).expect("the allowlist parses");
        let rendered = grants.render("aex").expect("a bare database name renders");

        assert_eq!(
            rendered.first().map(String::as_str),
            Some("REVOKE ALL ON DATABASE aex FROM PUBLIC"),
            "the database-wide denial runs before any grant"
        );
        for role in grants.role_names() {
            assert!(
                rendered.contains(&format!("GRANT CONNECT ON DATABASE aex TO {role}")),
                "`{role}` could not open a session"
            );
        }
        assert!(
            !rendered
                .iter()
                .any(|statement| statement.starts_with("GRANT") && statement.ends_with("PUBLIC")),
            "granting anything back to PUBLIC would undo the revoke's purpose"
        );
    }

    #[test]
    fn the_render_is_revocations_first_stable_and_one_statement_per_string() {
        let grants = GrantSet::load(grants_path()).expect("the allowlist parses");
        let rendered = grants.render("aex").expect("renders");
        let first_grant = rendered
            .iter()
            .position(|statement| statement.starts_with("GRANT "))
            .expect("something is granted");
        let last_revoke = rendered
            .iter()
            .rposition(|statement| statement.starts_with("REVOKE "))
            .expect("something is revoked");
        assert!(
            last_revoke < first_grant,
            "a revoke after a grant would take back what this run just handed out"
        );
        assert_eq!(rendered, grants.render("aex").expect("renders"));
        assert!(
            rendered
                .iter()
                .all(|statement| !statement.contains(';') && !statement.contains("--")),
            "nothing can ride along inside a rendered statement"
        );
    }

    #[test]
    fn the_epoch_table_and_the_audit_log_are_unwritable_by_construction() {
        for (from, to) in [
            (
                "\"control.authorization_epoch:SELECT\",\n  \"control.credential_pepper:SELECT\",\n  \"identity.user:SELECT\",",
                "\"control.authorization_epoch:SELECT,UPDATE\",",
            ),
            (
                "\"control.audit_event:SELECT,INSERT\",\n  \"control.outbox_message:SELECT,INSERT\",",
                "\"control.audit_event:SELECT,INSERT,UPDATE\",",
            ),
        ] {
            let error = edited(from, to);
            assert!(
                matches!(error, GrantError::Boundary { .. }),
                "expected a boundary refusal, got {error}"
            );
        }
    }

    #[test]
    fn the_read_only_role_cannot_be_given_a_write() {
        let error = edited(
            "\"identity.user:SELECT\",\n  \"identity.dashboard_session:SELECT\",",
            "\"identity.user:SELECT,UPDATE\",",
        );
        assert!(matches!(error, GrantError::Boundary { .. }), "{error}");
    }

    #[test]
    fn a_table_outside_the_roles_own_schemas_is_refused() {
        let error = edited(
            "\"finance.receipt_outbox:SELECT,UPDATE\"",
            "\"control.api_key:SELECT\"",
        );
        assert!(matches!(error, GrantError::Table { .. }), "{error}");
    }

    #[test]
    fn the_generic_epoch_bump_can_never_be_granted() {
        let error = edited(
            "functions = [\"control.bump_user_epoch(uuid)\"]",
            "functions = [\"control.bump_epoch(text, uuid)\"]",
        );
        assert!(matches!(error, GrantError::Boundary { .. }), "{error}");
    }

    #[test]
    fn an_undeclared_function_is_not_grantable() {
        let error = edited(
            "functions = [\"control.bump_user_epoch(uuid)\"]",
            "functions = [\"control.bump_nothing(uuid)\"]",
        );
        assert!(matches!(error, GrantError::Function(_)), "{error}");
    }

    #[test]
    fn a_database_name_that_is_not_a_bare_identifier_never_reaches_sql() {
        let grants = GrantSet::load(grants_path()).expect("the allowlist parses");
        for hostile in ["aex; DROP DATABASE aex", "\"aex\"", "Aex", "", "aex-dev"] {
            grants
                .render(hostile)
                .expect_err("only a bare lower-case name renders");
        }
        grants
            .render("aex_0123456789abcdef0123456789abcdef")
            .expect("a generated fixture database name renders");
    }

    #[test]
    fn declarative_grants_never_mutate_the_journal_or_let_costs_charge() {
        let grants = GrantSet::load(grants_path()).expect("the allowlist parses");
        let provider = grants.role("aex_provider_cost").expect("the cost role");
        assert_eq!(
            provider.tables,
            ["finance.provider_cost_fact:SELECT,INSERT"]
        );
        for (from, to) in [
            (
                "\"finance.journal_posting:SELECT,INSERT\",\n  \"finance.billing_account:SELECT,INSERT,UPDATE\",",
                "\"finance.journal_posting:SELECT,INSERT,DELETE\",",
            ),
            (
                "tables = [\"finance.provider_cost_fact:SELECT,INSERT\"]",
                "tables = [\"finance.account_balance:SELECT,INSERT,UPDATE\"]",
            ),
        ] {
            let error = edited(from, to);
            assert!(matches!(error, GrantError::Boundary { .. }), "{error}");
        }
    }

    #[test]
    fn an_unsupported_document_version_is_refused() {
        let error = edited("schema_version = 2", "schema_version = 1");
        assert!(matches!(error, GrantError::Version(1)), "{error}");
    }

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
