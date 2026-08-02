//! The online half of the one-shot task: migrate, verify, grants, backfill and
//! repair, all under the outer advisory lock.
//!
//! Every subcommand takes the lock first, so a second task exits `11` instead of
//! interleaving with the first. Transactional migrations roll back on a crash
//! and rerunning the exact artifact is safe; a non-transactional file is
//! exceptional and reaches the operator through a declared precondition and a
//! sibling repair file, never through improvisation.

use sqlx::Row as _;
use sqlx::postgres::PgConnection;

use crate::grants::GrantSet;

/// The dedicated history schema and table.
pub const HISTORY_SCHEMA: &str = "schema_admin";
/// The history table, qualified.
pub const HISTORY_TABLE: &str = "schema_admin._sqlx_migrations";

/// Why an online operation did not complete.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// A checksum or history row disagrees with the bundle.
    #[error("the applied history disagrees with the bundle: {0}")]
    ChecksumDrift(String),
    /// The applied head is not what the release expected.
    #[error("the applied head is `{found:?}`, not the expected `{expected}`")]
    HeadMismatch {
        /// What the database holds.
        found: Option<i64>,
        /// What the release asserted.
        expected: i64,
    },
    /// A declared precondition detected a partial non-transactional object.
    #[error("migration `{0}` left a partial object; run `repair --migration {0}`")]
    PreconditionFailed(i64),
    /// The actual grants differ from the declarative allowlist.
    #[error("the applied grants differ from grants.toml: {0}")]
    GrantDrift(String),
    /// A conservation law does not hold.
    #[error("the journal does not conserve: {0}")]
    Conservation(String),
    /// The database refused a statement.
    #[error("the database refused the operation: {0}")]
    Database(String),
}

/// The head the database has actually applied.
///
/// # Errors
///
/// Returns [`RunnerError::Database`] when the history cannot be read. An absent
/// history schema is not an error: a clean database has applied nothing.
pub async fn applied_head(connection: &mut PgConnection) -> Result<Option<i64>, RunnerError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
          WHERE table_schema = $1 AND table_name = '_sqlx_migrations')",
    )
    .bind(HISTORY_SCHEMA)
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if !exists {
        return Ok(None);
    }
    // The relation is this crate's own constant, not caller input.
    let head: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT max(version) FROM {HISTORY_TABLE} WHERE success"
    )))
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    Ok(head)
}

/// Fails unless the applied head is exactly `expected`.
///
/// # Errors
///
/// Returns [`RunnerError::HeadMismatch`] before any mutation, which is the point:
/// an out-of-order release must not apply a single statement.
pub async fn expect_applied_head(
    connection: &mut PgConnection,
    expected: i64,
) -> Result<(), RunnerError> {
    let found = applied_head(connection).await?;
    if found == Some(expected) {
        Ok(())
    } else {
        Err(RunnerError::HeadMismatch { found, expected })
    }
}

/// Applies every pending migration under the already-held outer lock.
///
/// # Errors
///
/// Returns [`RunnerError::ChecksumDrift`] when a version's checksum differs
/// from the bundle or a previously applied version is missing from it, and
/// [`RunnerError::Database`] for a refused statement. `sqlx` keeps
/// `ignore_missing` off, so drift fails closed rather than being reinterpreted.
pub async fn migrate(
    connection: &mut PgConnection,
    migrator: &sqlx::migrate::Migrator,
) -> Result<(), RunnerError> {
    migrator.run(&mut *connection).await.map_err(|error| {
        let rendered = error.to_string();
        if rendered.contains("checksum") || rendered.contains("was previously applied") {
            RunnerError::ChecksumDrift(rendered)
        } else {
            RunnerError::Database(rendered)
        }
    })
}

/// The two conservation laws, read-only.
///
/// # Errors
///
/// Returns [`RunnerError::Conservation`] when the projection disagrees with the
/// journal for any account, or when the global signed sum is not zero. The
/// journal wins on divergence; this command reports and never repairs.
pub async fn check_conservation(connection: &mut PgConnection) -> Result<(), RunnerError> {
    let divergent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance.account_balance b \
           LEFT JOIN (SELECT account_id, sum(amount_microusd) AS s \
                        FROM finance.journal_posting GROUP BY account_id) j \
             ON j.account_id = b.account_id \
          WHERE b.balance_microusd IS DISTINCT FROM coalesce(j.s, 0)",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if divergent != 0 {
        return Err(RunnerError::Conservation(format!(
            "{divergent} account(s) diverge from the journal"
        )));
    }
    let global: Option<i64> =
        sqlx::query_scalar("SELECT sum(amount_microusd) FROM finance.journal_posting")
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| RunnerError::Database(error.to_string()))?;
    match global {
        None | Some(0) => Ok(()),
        Some(imbalance) => Err(RunnerError::Conservation(format!(
            "the global posting sum is {imbalance} micro-USD, not zero"
        ))),
    }
}

/// One difference between the declarative allowlist and the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantDiff {
    /// The role the difference is about, or `PUBLIC` for a baseline denial.
    pub role: String,
    /// The qualified database object.
    pub object: String,
    /// The privilege.
    pub privilege: String,
    /// Whether the allowlist declares it.
    pub declared: bool,
    /// Whether the database holds it.
    pub actual: bool,
}

/// Diffs the whole declarative allowlist against `PostgreSQL`'s privilege probes.
///
/// # Errors
///
/// Returns [`RunnerError::Database`] when the privilege probe fails.
#[allow(
    clippy::too_many_lines,
    reason = "one ordered probe per schema-v2 privilege class keeps verification visibly complete"
)]
pub async fn diff_grants(
    connection: &mut PgConnection,
    grants: &GrantSet,
) -> Result<Vec<GrantDiff>, RunnerError> {
    let mut diffs = Vec::new();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;
    let relations: Vec<String> = sqlx::query(
        "SELECT table_schema || '.' || table_name AS relation \
           FROM information_schema.tables \
          WHERE table_schema IN ('identity','control','finance')",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?
    .into_iter()
    .map(|row| row.get::<String, _>("relation"))
    .collect();

    if grants.database_policy().revoke_public {
        for privilege in ["CONNECT", "CREATE", "TEMPORARY"] {
            let actual: bool = sqlx::query_scalar(
                "SELECT EXISTS (\
                   SELECT 1 FROM pg_database d \
                   CROSS JOIN LATERAL aclexplode(\
                     coalesce(d.datacl, acldefault('d', d.datdba))\
                   ) acl \
                   WHERE d.datname = current_database() \
                     AND acl.grantee = 0 AND acl.privilege_type = $1\
                 )",
            )
            .bind(privilege)
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| RunnerError::Database(error.to_string()))?;
            if actual {
                diffs.push(GrantDiff {
                    role: "PUBLIC".to_owned(),
                    object: format!("DATABASE {database}"),
                    privilege: privilege.to_owned(),
                    declared: false,
                    actual,
                });
            }
        }
    }
    for schema in grants
        .schemas()
        .iter()
        .filter(|schema| schema.revoke_public)
    {
        for privilege in ["USAGE", "CREATE"] {
            let actual: bool = sqlx::query_scalar(
                "SELECT EXISTS (\
                   SELECT 1 FROM pg_namespace n \
                   CROSS JOIN LATERAL aclexplode(\
                     coalesce(n.nspacl, acldefault('n', n.nspowner))\
                   ) acl \
                   WHERE n.nspname = $1 \
                     AND acl.grantee = 0 AND acl.privilege_type = $2\
                 )",
            )
            .bind(&schema.name)
            .bind(privilege)
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| RunnerError::Database(error.to_string()))?;
            if actual {
                diffs.push(GrantDiff {
                    role: "PUBLIC".to_owned(),
                    object: format!("SCHEMA {}", schema.name),
                    privilege: privilege.to_owned(),
                    declared: false,
                    actual,
                });
            }
        }
    }
    for function in grants
        .functions()
        .iter()
        .filter(|function| function.revoke_public)
    {
        let actual: bool = sqlx::query_scalar(
            "SELECT EXISTS (\
               SELECT 1 FROM pg_proc p \
               CROSS JOIN LATERAL aclexplode(\
                 coalesce(p.proacl, acldefault('f', p.proowner))\
               ) acl \
               WHERE p.oid = to_regprocedure($1) \
                 AND acl.grantee = 0 AND acl.privilege_type = 'EXECUTE'\
             )",
        )
        .bind(&function.name)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;
        if actual {
            diffs.push(GrantDiff {
                role: "PUBLIC".to_owned(),
                object: format!("FUNCTION {}", function.name),
                privilege: "EXECUTE".to_owned(),
                declared: false,
                actual,
            });
        }
    }

    for role in grants.roles() {
        for privilege in ["CONNECT", "CREATE", "TEMPORARY"] {
            let declared = privilege == "CONNECT" && role.connect;
            let actual: bool =
                sqlx::query_scalar("SELECT has_database_privilege($1, current_database(), $2)")
                    .bind(&role.name)
                    .bind(privilege)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(|error| RunnerError::Database(error.to_string()))?;
            if declared != actual {
                diffs.push(GrantDiff {
                    role: role.name.clone(),
                    object: format!("DATABASE {database}"),
                    privilege: privilege.to_owned(),
                    declared,
                    actual,
                });
            }
        }
        for schema in grants.schemas() {
            let declared = role.schemas.contains(&schema.name);
            let actual: bool = sqlx::query_scalar("SELECT has_schema_privilege($1, $2, 'USAGE')")
                .bind(&role.name)
                .bind(&schema.name)
                .fetch_one(&mut *connection)
                .await
                .map_err(|error| RunnerError::Database(error.to_string()))?;
            if declared != actual {
                diffs.push(GrantDiff {
                    role: role.name.clone(),
                    object: format!("SCHEMA {}", schema.name),
                    privilege: "USAGE".to_owned(),
                    declared,
                    actual,
                });
            }
        }
        for function in grants.functions() {
            let declared = role.functions.contains(&function.name);
            let actual: bool =
                sqlx::query_scalar("SELECT has_function_privilege($1, $2, 'EXECUTE')")
                    .bind(&role.name)
                    .bind(&function.name)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(|error| RunnerError::Database(error.to_string()))?;
            if declared != actual {
                diffs.push(GrantDiff {
                    role: role.name.clone(),
                    object: format!("FUNCTION {}", function.name),
                    privilege: "EXECUTE".to_owned(),
                    declared,
                    actual,
                });
            }
        }
        for relation in &relations {
            for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
                let declared = grants.declares(&role.name, relation, privilege);
                let actual: bool = sqlx::query_scalar("SELECT has_table_privilege($1, $2, $3)")
                    .bind(&role.name)
                    .bind(relation)
                    .bind(privilege)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(|error| RunnerError::Database(error.to_string()))?;
                if declared != actual {
                    diffs.push(GrantDiff {
                        role: role.name.clone(),
                        object: format!("TABLE {relation}"),
                        privilege: privilege.to_owned(),
                        declared,
                        actual,
                    });
                }
            }
        }
    }
    Ok(diffs)
}

/// Applies the canonical v2 grant renderer, one statement at a time.
///
/// # Errors
///
/// Returns [`RunnerError::GrantDrift`] when the committed allowlist cannot
/// render and [`RunnerError::Database`] when `PostgreSQL` refuses a statement.
pub async fn apply_grants(
    connection: &mut PgConnection,
    grants: &GrantSet,
) -> Result<(), RunnerError> {
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;
    let statements = grants
        .render(&database)
        .map_err(|error| RunnerError::GrantDrift(error.to_string()))?;
    for statement in statements {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&mut *connection)
            .await
            .map_err(|error| RunnerError::Database(error.to_string()))?;
    }
    Ok(())
}

/// Runs a non-transactional migration's declared precondition.
///
/// # Errors
///
/// Returns [`RunnerError::PreconditionFailed`] when the query detects a partial
/// object, which directs the operator to the sibling repair file rather than to
/// an improvised fix.
pub async fn check_precondition(
    connection: &mut PgConnection,
    version: i64,
    precondition: &str,
) -> Result<(), RunnerError> {
    // The precondition is the migration header's own committed query.
    let detected: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(precondition.to_owned()))
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;
    if detected {
        Err(RunnerError::PreconditionFailed(version))
    } else {
        Ok(())
    }
}

/// Resumes a keyset-paged backfill from its durable cursor.
///
/// # Errors
///
/// Returns [`RunnerError::Database`] when the cursor table cannot be read or
/// written. The cursor is what makes a long backfill survive task replacement.
pub async fn backfill_cursor(
    connection: &mut PgConnection,
    version: i64,
) -> Result<Option<String>, RunnerError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
          WHERE table_schema = $1 AND table_name = 'backfill_cursor')",
    )
    .bind(HISTORY_SCHEMA)
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if !exists {
        return Ok(None);
    }
    sqlx::query_scalar(
        "SELECT cursor FROM schema_admin.backfill_cursor WHERE migration_version = $1",
    )
    .bind(version)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_SCHEMA, HISTORY_TABLE, RunnerError};

    #[test]
    fn history_lives_outside_every_application_search_path() {
        assert_eq!(HISTORY_SCHEMA, "schema_admin");
        assert_eq!(HISTORY_TABLE, "schema_admin._sqlx_migrations");
        assert!(!HISTORY_TABLE.starts_with("public"));
    }

    #[test]
    fn a_head_mismatch_names_both_sides_so_the_release_can_be_diagnosed() {
        let error = RunnerError::HeadMismatch {
            found: Some(20_260_801_000_400),
            expected: 20_260_801_000_600,
        };
        let rendered = error.to_string();
        assert!(rendered.contains("20260801000400"));
        assert!(rendered.contains("20260801000600"));
    }

    #[test]
    fn a_failed_precondition_directs_the_operator_to_the_repair_path() {
        let error = RunnerError::PreconditionFailed(20_260_801_000_700);
        assert!(error.to_string().contains("repair --migration"));
    }
}
