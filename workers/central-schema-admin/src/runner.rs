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
    /// The pepper row a seed named exists and says something else.
    #[error("the pepper row cannot be seeded: {0}")]
    PepperConflict(String),
    /// The assertion signing-key row exists, or another key is already active.
    #[error("the signing key cannot be seeded: {0}")]
    SigningKeyConflict(String),
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

/// Seeds or reconciles the exact qualified Lambda alias Aurora invokes.
///
/// This is deployment data, not migration data: the physical function name is
/// plane-owned. It runs in the same locked schema-admin task immediately after
/// migration, before that task can report the plane ready.
///
/// # Errors
///
/// Returns [`RunnerError::Database`] when `PostgreSQL` refuses the binding.
pub async fn configure_outbox_wake(
    connection: &mut PgConnection,
    lambda_arn: &str,
) -> Result<(), RunnerError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS aws_lambda CASCADE")
        .execute(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;
    let dry_run: i32 = sqlx::query_scalar(
        "SELECT status_code \
           FROM aws_lambda.invoke($1::text, '{}'::json, NULL::text, 'DryRun'::text)",
    )
    .bind(lambda_arn)
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if dry_run != 204 {
        return Err(RunnerError::Database(format!(
            "the outbox wake Lambda dry-run returned {dry_run}, not 204"
        )));
    }
    sqlx::query(
        "INSERT INTO control.outbox_wake_target (singleton, lambda_arn) \
         VALUES (true, $1) \
         ON CONFLICT (singleton) DO UPDATE SET lambda_arn = EXCLUDED.lambda_arn",
    )
    .bind(lambda_arn)
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(|error| RunnerError::Database(error.to_string()))
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

/// The four credential-pepper rows that can exist, and nothing else.
///
/// Both planes hold a `credential_pepper` table and `cursor` appears in both,
/// so a `--schema`/`--purpose` pair would admit combinations no `CHECK`
/// constraint allows. Enumerating the legal rows instead makes an illegal one
/// unrepresentable rather than validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PepperRow {
    /// `identity.credential_pepper`, purpose `identity` — account credentials.
    Identity,
    /// `identity.credential_pepper`, purpose `cursor` — identity page cursors.
    IdentityCursor,
    /// `control.credential_pepper`, purpose `api_key` — workspace API keys.
    ApiKey,
    /// `control.credential_pepper`, purpose `cursor` — control page cursors.
    ControlCursor,
}

impl PepperRow {
    /// Every row, for the totality test.
    pub const ALL: [Self; 4] = [
        Self::Identity,
        Self::IdentityCursor,
        Self::ApiKey,
        Self::ControlCursor,
    ];

    /// The qualified relation the row lives in.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            Self::Identity | Self::IdentityCursor => "identity.credential_pepper",
            Self::ApiKey | Self::ControlCursor => "control.credential_pepper",
        }
    }

    /// The `purpose` value, exactly as the table's `CHECK` spells it.
    #[must_use]
    pub const fn purpose(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::IdentityCursor | Self::ControlCursor => "cursor",
            Self::ApiKey => "api_key",
        }
    }
}

/// What a seed found or did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PepperSeeded {
    /// The row did not exist and now does.
    Inserted,
    /// The row already existed and already says exactly this.
    AlreadyExact,
}

/// Seeds one active credential-pepper lifecycle row.
///
/// # Why this is not a migration
///
/// `secret_ref` is a Secrets Manager **version id**, minted when the plane
/// created the secret version. No file in this repository can know it, and a
/// baseline `INSERT` carrying a literal would bake one plane's identifier into
/// an immutable migration every plane applies. The row is therefore deployment
/// data seeded by the release path — the same one-shot that applies the schema
/// and reconciles the grants — rather than schema.
///
/// Without it `central-api` refuses to start: it probes the active pepper for
/// each purpose before it binds a listener, and `api_key_create` reads the
/// active `api_key` row to mint at all.
///
/// # Idempotence and its limit
///
/// Re-running with identical arguments answers [`PepperSeeded::AlreadyExact`].
/// Anything else — a different `secret_ref` under the same version, a retired
/// row, or another version already active for the purpose — is refused. A
/// rotation is a ceremony with two live versions, not a seed, and quietly
/// re-pointing a version at other material would make every credential
/// fingerprinted under it unverifiable while reporting success.
///
/// # Errors
///
/// Returns [`RunnerError::PepperConflict`] when a row exists and disagrees, and
/// [`RunnerError::Database`] when a statement is refused.
pub async fn seed_pepper(
    connection: &mut PgConnection,
    row: PepperRow,
    version: i16,
    secret_ref: &str,
) -> Result<PepperSeeded, RunnerError> {
    // The relation and purpose come from a closed enum in this crate, never
    // from caller text; `version` and `secret_ref` are bound.
    let existing: Option<(String, String, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT purpose, secret_ref, state FROM {} WHERE version = $1",
            row.table()
        )))
        .bind(version)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| RunnerError::Database(error.to_string()))?;

    if let Some((purpose, stored_ref, state)) = existing {
        return if purpose == row.purpose()
            && stored_ref == secret_ref
            && state.as_deref() == Some("active")
        {
            Ok(PepperSeeded::AlreadyExact)
        } else {
            Err(RunnerError::PepperConflict(format!(
                "{} version {version} is `{purpose}`/`{}` and cannot be re-seeded as `{}`/active",
                row.table(),
                state.as_deref().unwrap_or("unknown"),
                row.purpose()
            )))
        };
    }

    // The partial unique index admits one active row per purpose. Naming the
    // occupant is the difference between "seed this plane" and "rotate", which
    // is a ceremony this command deliberately cannot perform.
    let active: Option<i16> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT version FROM {} WHERE purpose = $1 AND state = 'active'",
        row.table()
    )))
    .bind(row.purpose())
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if let Some(occupant) = active {
        return Err(RunnerError::PepperConflict(format!(
            "{} already has version {occupant} active for `{}`; a rotation is not a seed",
            row.table(),
            row.purpose()
        )));
    }

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {} (version, purpose, state, secret_ref, created_at) \
         VALUES ($1, $2, 'active', $3, now())",
        row.table()
    )))
    .bind(version)
    .bind(row.purpose())
    .bind(secret_ref)
    .execute(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    Ok(PepperSeeded::Inserted)
}

/// What an assertion signing-key seed found or did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningKeySeeded {
    /// The row did not exist and now does.
    Inserted,
    /// The row already existed and already says exactly this.
    AlreadyExact,
}

#[derive(sqlx::FromRow)]
struct StoredSigningKey {
    alg: String,
    public_key: Vec<u8>,
    secret_ref: String,
    state: String,
    activates_at_ms: i64,
    retires_at_ms: i64,
    retired_at: Option<String>,
}

/// Seeds the one active Ed25519 signing-key lifecycle row.
///
/// # Why this is not a migration
///
/// The public half, key id, lifecycle window and Secrets Manager name are
/// properties of plane material created outside the source bundle. A migration
/// cannot carry one plane's key, and the release must not write this authority
/// through raw SQL. `secret_ref` is deliberately the secret **name**: the
/// authorizer compares it with its configured secret id before reading the
/// private seed. It is not the version id used by credential-pepper rows.
///
/// # Idempotence and its limit
///
/// Replaying the exact row is a no-op. Any disagreement under the same `kid`,
/// or another active key, is refused. Introducing a successor while the first
/// remains active is rotation and needs its own overlap ceremony.
///
/// # Errors
///
/// Returns [`RunnerError::SigningKeyConflict`] for drift or a second active
/// key, and [`RunnerError::Database`] when `PostgreSQL` refuses a statement.
pub async fn seed_signing_key(
    connection: &mut PgConnection,
    kid: uuid::Uuid,
    public_key: [u8; 32],
    secret_ref: &str,
    activates_at_ms: i64,
    retires_at_ms: i64,
) -> Result<SigningKeySeeded, RunnerError> {
    let existing: Option<StoredSigningKey> = sqlx::query_as(
        "SELECT alg, public_key, secret_ref, state, \
                    (extract(epoch FROM activates_at) * 1000)::bigint AS activates_at_ms, \
                    (extract(epoch FROM retires_at) * 1000)::bigint AS retires_at_ms, \
                    retired_at::text AS retired_at \
               FROM control.signing_key WHERE kid = $1",
    )
    .bind(kid)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    if let Some(stored) = existing {
        return if stored.alg == "ed25519"
            && stored.public_key == public_key
            && stored.secret_ref == secret_ref
            && stored.state == "active"
            && stored.activates_at_ms == activates_at_ms
            && stored.retires_at_ms == retires_at_ms
            && stored.retired_at.is_none()
        {
            Ok(SigningKeySeeded::AlreadyExact)
        } else {
            Err(RunnerError::SigningKeyConflict(format!(
                "kid {kid} already exists as `{}`/`{}` and does not match the admitted active key",
                stored.alg, stored.state,
            )))
        };
    }

    let active: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT kid FROM control.signing_key WHERE state = 'active'")
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| RunnerError::Database(error.to_string()))?;
    if let Some(occupant) = active {
        return Err(RunnerError::SigningKeyConflict(format!(
            "kid {occupant} is already active; introducing {kid} is a rotation, not a seed"
        )));
    }

    sqlx::query(
        "INSERT INTO control.signing_key \
           (kid, alg, public_key, secret_ref, state, created_at, activates_at, retires_at) \
         VALUES ($1, 'ed25519', $2, $3, 'active', now(), \
                 TIMESTAMPTZ 'epoch' + $4 * INTERVAL '1 millisecond', \
                 TIMESTAMPTZ 'epoch' + $5 * INTERVAL '1 millisecond')",
    )
    .bind(kid)
    .bind(public_key.as_slice())
    .bind(secret_ref)
    .bind(activates_at_ms)
    .bind(retires_at_ms)
    .execute(&mut *connection)
    .await
    .map_err(|error| RunnerError::Database(error.to_string()))?;
    Ok(SigningKeySeeded::Inserted)
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_SCHEMA, HISTORY_TABLE, PepperRow, RunnerError};

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

    #[test]
    fn every_pepper_row_names_a_relation_and_a_purpose_its_check_admits() {
        // The two tables spell their allowed purposes differently and share
        // exactly one. A row naming the wrong pair would be refused by the
        // database at deploy time rather than here.
        for row in PepperRow::ALL {
            match row.table() {
                "identity.credential_pepper" => {
                    assert!(matches!(row.purpose(), "identity" | "cursor"), "{row:?}");
                }
                "control.credential_pepper" => {
                    assert!(matches!(row.purpose(), "api_key" | "cursor"), "{row:?}");
                }
                other => panic!("`{other}` is not a credential-pepper relation"),
            }
        }
        let mut pairs: Vec<(&str, &str)> = PepperRow::ALL
            .iter()
            .map(|row| (row.table(), row.purpose()))
            .collect();
        let count = pairs.len();
        pairs.sort_unstable();
        pairs.dedup();
        assert_eq!(pairs.len(), count, "two rows describe one pepper");
    }
}
