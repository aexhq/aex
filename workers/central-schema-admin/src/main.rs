//! One-shot central schema administration task.
//!
//! Every schema fact lives in the library beside this file. The binary is the
//! CLI, the exit contract and the receipt, and nothing else.

use std::path::PathBuf;

use aex_wire::canonical::to_jcs_string;
use central_schema_admin::ADVISORY_LOCK_KEY;
use central_schema_admin::connect::{
    ConnectError, Endpoint, apply_timeouts, connect, resolve_credential, take_outer_lock,
};
use central_schema_admin::grants::GrantSet;
use central_schema_admin::migration;
use central_schema_admin::migration::MigrationBundle;
use central_schema_admin::runner::{
    PepperRow, RunnerError, applied_head, apply_grants, backfill_cursor, check_conservation,
    diff_grants, expect_applied_head, migrate, seed_pepper,
};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

/// One-shot schema administration CLI.
#[derive(Debug, Parser)]
#[command(name = "central-schema-admin")]
struct Cli {
    /// Aurora `PostgreSQL` host.
    #[arg(long)]
    database_host: String,
    /// Aurora `PostgreSQL` port.
    #[arg(long)]
    database_port: u16,
    /// Database name.
    #[arg(long)]
    database_name: String,
    /// Secrets Manager ARN containing the DDL-only credential.
    #[arg(long)]
    database_secret_arn: String,
    /// Pinned RDS root CA bundle.
    #[arg(long)]
    tls_root_ca_path: PathBuf,
    /// Connection timeout.
    #[arg(long, default_value_t = 10_000)]
    connect_timeout_ms: u64,
    /// Remote plane.
    #[arg(long, value_enum)]
    plane: Plane,
    /// Admitted release identity.
    #[arg(long)]
    release: String,
    /// Emit the canonical machine receipt.
    #[arg(long)]
    json: bool,
    /// Operation.
    #[command(subcommand)]
    command: Command,
}

/// Remote plane names.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Plane {
    /// Development plane.
    Dev,
    /// Production plane.
    Prd,
}

/// Supported schema operations.
#[derive(Debug, Subcommand)]
enum Command {
    /// Validate and display the embedded migration plan without a database.
    Plan {
        /// Required current database head.
        #[arg(long)]
        expect_applied_head: Option<i64>,
    },
    /// Apply pending migrations.
    Migrate {
        /// Required bundle head.
        #[arg(long)]
        expect_head: i64,
        /// Required current database head.
        #[arg(long)]
        expect_applied_head: Option<i64>,
        /// Outer lock timeout.
        #[arg(long, default_value_t = 30_000)]
        lock_timeout_ms: u64,
        /// Statement deadline.
        #[arg(long, default_value_t = 900_000)]
        statement_timeout_ms: u64,
        /// Admit migrations marked destructive.
        #[arg(long, requires = "backup_evidence")]
        allow_destructive: bool,
        /// PITR/backup evidence identity.
        #[arg(long, requires = "allow_destructive")]
        backup_evidence: Option<String>,
    },
    /// Verify the applied schema and conservation laws.
    Verify {
        /// Required bundle head.
        #[arg(long)]
        expect_head: i64,
        /// Run journal/projection conservation checks.
        #[arg(long)]
        check_conservation: bool,
    },
    /// Compare or reconcile declarative grants.
    Grants {
        /// Fail on grant drift without mutation.
        #[arg(long, conflicts_with = "apply", required_unless_present = "apply")]
        check: bool,
        /// Reconcile the exact allowlist.
        #[arg(long, conflicts_with = "check", required_unless_present = "check")]
        apply: bool,
    },
    /// Continue a keyset-paged backfill.
    Backfill {
        /// Owning migration version.
        #[arg(long)]
        migration: i64,
        /// Rows per transaction.
        #[arg(long, default_value_t = 5_000)]
        batch_size: u32,
        /// Maximum batches; zero means until exhausted.
        #[arg(long, default_value_t = 0)]
        max_batches: u32,
    },
    /// Execute a committed sibling repair file.
    Repair {
        /// Failed non-transactional migration.
        #[arg(long)]
        migration: i64,
        /// Exact confirmation token printed by the failed precondition.
        #[arg(long)]
        confirm: String,
    },
    /// Seed one active credential-pepper lifecycle row.
    ///
    /// Deployment data rather than schema: the reference is a Secrets Manager
    /// version id the plane minted, so no migration can carry it. Nothing else
    /// writes these rows, and `central-api` refuses to start without them.
    SeedPepper {
        /// Which of the four pepper rows.
        #[arg(long, value_enum)]
        pepper: Pepper,
        /// The version the credential rows name.
        #[arg(long)]
        version: i16,
        /// The Secrets Manager **version id** holding that version's material.
        #[arg(long)]
        secret_ref: String,
    },
}

/// The four credential-pepper rows, as CLI values.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Pepper {
    /// `identity.credential_pepper` / `identity`.
    Identity,
    /// `identity.credential_pepper` / `cursor`.
    IdentityCursor,
    /// `control.credential_pepper` / `api_key`.
    ApiKey,
    /// `control.credential_pepper` / `cursor`.
    ControlCursor,
}

impl Pepper {
    const fn row(self) -> PepperRow {
        match self {
            Self::Identity => PepperRow::Identity,
            Self::IdentityCursor => PepperRow::IdentityCursor,
            Self::ApiKey => PepperRow::ApiKey,
            Self::ControlCursor => PepperRow::ControlCursor,
        }
    }
}

/// Stable process exit contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Exit {
    Success = 0,
    ChecksumDrift = 10,
    LockUnavailable = 11,
    HeadMismatch = 12,
    PreconditionFailed = 13,
    GrantDrift = 14,
    DestructiveEvidenceMissing = 15,
    Connection = 20,
    SecretUnavailable = 21,
    Conservation = 30,
}

impl Exit {
    const ALL: [Self; 10] = [
        Self::Success,
        Self::ChecksumDrift,
        Self::LockUnavailable,
        Self::HeadMismatch,
        Self::PreconditionFailed,
        Self::GrantDrift,
        Self::DestructiveEvidenceMissing,
        Self::Connection,
        Self::SecretUnavailable,
        Self::Conservation,
    ];
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt<'a> {
    release_id: &'a str,
    plane: &'a str,
    bundle_head: i64,
    applied_head_before: Option<i64>,
    applied_head_after: Option<i64>,
    migration_versions: Vec<i64>,
    grant_roles: Vec<&'a str>,
    lock_key: i64,
    conservation: &'a str,
}

/// Maps a connection failure onto the exit contract.
const fn connect_exit(error: &ConnectError) -> Exit {
    match error {
        ConnectError::SecretUnavailable(_) | ConnectError::SecretShape => Exit::SecretUnavailable,
        ConnectError::RootCaUnreadable(_) | ConnectError::Connection(_) => Exit::Connection,
        ConnectError::LockUnavailable => Exit::LockUnavailable,
    }
}

/// Maps an online failure onto the exit contract.
const fn runner_exit(error: &RunnerError) -> Exit {
    match error {
        RunnerError::ChecksumDrift(_) => Exit::ChecksumDrift,
        RunnerError::HeadMismatch { .. } => Exit::HeadMismatch,
        // A pepper row that exists and disagrees is a precondition the release
        // asserted and the database refutes, which is the same answer as a
        // partial migration object: exit `13`. Sharing it keeps the exit
        // contract at the ten codes a release script already knows.
        RunnerError::PreconditionFailed(_) | RunnerError::PepperConflict(_) => {
            Exit::PreconditionFailed
        }
        RunnerError::GrantDrift(_) => Exit::GrantDrift,
        RunnerError::Conservation(_) => Exit::Conservation,
        RunnerError::Database(_) => Exit::Connection,
    }
}

/// The endpoint this invocation addresses.
fn endpoint(cli: &Cli) -> Endpoint {
    Endpoint {
        host: cli.database_host.clone(),
        port: cli.database_port,
        database: cli.database_name.clone(),
        tls_root_ca_path: cli.tls_root_ca_path.clone(),
        connect_timeout: std::time::Duration::from_millis(cli.connect_timeout_ms),
    }
}

/// Opens the DDL session and takes the outer lock.
///
/// Every mutating and verifying command goes through here, so "exactly one task
/// per admitted migration" is a property of the runner rather than of the
/// deployment configuration.
async fn open(cli: &Cli) -> Result<sqlx::postgres::PgConnection, (Exit, String)> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let secrets = aws_sdk_secretsmanager::Client::new(&aws);
    let credential = resolve_credential(&secrets, &cli.database_secret_arn)
        .await
        .map_err(|error| (connect_exit(&error), error.to_string()))?;
    let mut connection = connect(&endpoint(cli), &credential)
        .await
        .map_err(|error| (connect_exit(&error), error.to_string()))?;
    take_outer_lock(&mut connection)
        .await
        .map_err(|error| (connect_exit(&error), error.to_string()))?;
    Ok(connection)
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm per subcommand; splitting the tree would hide the exit contract"
)]
async fn run(cli: &Cli) -> Result<String, (Exit, String)> {
    debug_assert_eq!(Exit::ALL.len(), 10);
    let bundle =
        MigrationBundle::embedded().map_err(|error| (Exit::ChecksumDrift, error.to_string()))?;
    let migrator = migration::native_migrator();
    let grant_set = GrantSet::embedded().map_err(|error| (Exit::GrantDrift, error.to_string()))?;
    grant_set
        .role("aex_provider_cost")
        .ok_or_else(|| (Exit::GrantDrift, "provider-cost role is absent".to_owned()))?;
    let plane = match cli.plane {
        Plane::Dev => "dev",
        Plane::Prd => "prd",
    };
    let receipt = |before: Option<i64>, after: Option<i64>, conservation: &'static str| Receipt {
        release_id: &cli.release,
        plane,
        bundle_head: bundle.head(),
        applied_head_before: before,
        applied_head_after: after,
        migration_versions: bundle.versions(),
        grant_roles: grant_set.role_names(),
        lock_key: ADVISORY_LOCK_KEY,
        conservation,
    };
    let render = |value: &Receipt<'_>| {
        to_jcs_string(value).map_err(|error| (Exit::ChecksumDrift, error.to_string()))
    };

    match &cli.command {
        // `plan` is deliberately offline: it validates the artifact a release is
        // about to run without holding a credential or a lock.
        Command::Plan {
            expect_applied_head,
        } => render(&receipt(
            *expect_applied_head,
            *expect_applied_head,
            "not_requested",
        )),

        Command::Migrate {
            expect_head,
            expect_applied_head: expected_applied,
            lock_timeout_ms,
            statement_timeout_ms,
            allow_destructive,
            backup_evidence,
        } => {
            if *expect_head != bundle.head() {
                return Err((
                    Exit::HeadMismatch,
                    format!(
                        "the bundle head is {}, not the expected {expect_head}",
                        bundle.head()
                    ),
                ));
            }
            if *allow_destructive && backup_evidence.as_deref().is_none_or(str::is_empty) {
                return Err((
                    Exit::DestructiveEvidenceMissing,
                    "a destructive migration needs recorded backup evidence".to_owned(),
                ));
            }
            if bundle.has_destructive() && !*allow_destructive {
                return Err((
                    Exit::DestructiveEvidenceMissing,
                    "the bundle declares a destructive migration; pass --allow-destructive with \
                     --backup-evidence"
                        .to_owned(),
                ));
            }
            let mut connection = open(cli).await?;
            apply_timeouts(&mut connection, *lock_timeout_ms, *statement_timeout_ms)
                .await
                .map_err(|error| (connect_exit(&error), error.to_string()))?;
            if let Some(expected) = expected_applied {
                expect_applied_head(&mut connection, *expected)
                    .await
                    .map_err(|error| (runner_exit(&error), error.to_string()))?;
            }
            let before = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            migrate(&mut connection, &migrator)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            let after = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            render(&receipt(before, after, "not_requested"))
        }

        Command::Verify {
            expect_head,
            check_conservation: conservation,
        } => {
            if *expect_head != bundle.head() {
                return Err((
                    Exit::HeadMismatch,
                    format!(
                        "the bundle head is {}, not the expected {expect_head}",
                        bundle.head()
                    ),
                ));
            }
            let mut connection = open(cli).await?;
            let applied = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            if applied != Some(bundle.head()) {
                return Err((
                    Exit::HeadMismatch,
                    format!("the applied head is {applied:?}, not the bundle head {expect_head}"),
                ));
            }
            let diffs = diff_grants(&mut connection, &grant_set)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            if !diffs.is_empty() {
                return Err((
                    Exit::GrantDrift,
                    format!("{} grant difference(s) against grants.toml", diffs.len()),
                ));
            }
            let verdict = if *conservation {
                check_conservation(&mut connection)
                    .await
                    .map_err(|error| (runner_exit(&error), error.to_string()))?;
                "holds"
            } else {
                "not_requested"
            };
            render(&receipt(applied, applied, verdict))
        }

        Command::Grants { check, apply } => {
            let mut connection = open(cli).await?;
            let diffs = diff_grants(&mut connection, &grant_set)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            if *check && !diffs.is_empty() {
                return Err((
                    Exit::GrantDrift,
                    format!("{} grant difference(s) against grants.toml", diffs.len()),
                ));
            }
            if *apply {
                apply_grants(&mut connection, &grant_set)
                    .await
                    .map_err(|error| (runner_exit(&error), error.to_string()))?;
            }
            let applied = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            render(&receipt(applied, applied, "not_requested"))
        }

        Command::Backfill { migration, .. } => {
            if !bundle.versions().contains(migration) {
                return Err((
                    Exit::HeadMismatch,
                    format!("`{migration}` is not a bundled migration"),
                ));
            }
            let mut connection = open(cli).await?;
            // The durable cursor is the resumption point that lets a long
            // backfill survive task replacement. The body itself belongs to the
            // migration that declares one, and no bundled migration does yet.
            let _cursor = backfill_cursor(&mut connection, *migration)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            let applied = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            render(&receipt(applied, applied, "not_requested"))
        }

        Command::Repair { migration, confirm } => {
            let repair = bundle
                .repair_sql(*migration)
                .map_err(|error| (Exit::PreconditionFailed, error.to_string()))?;
            if confirm != &MigrationBundle::repair_token(*migration) {
                return Err((
                    Exit::PreconditionFailed,
                    "the confirmation token does not match this repair".to_owned(),
                ));
            }
            let mut connection = open(cli).await?;
            sqlx::raw_sql(sqlx::AssertSqlSafe(repair))
                .execute(&mut connection)
                .await
                .map_err(|error| (Exit::Connection, error.to_string()))?;
            let applied = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            render(&receipt(applied, applied, "not_requested"))
        }

        Command::SeedPepper {
            pepper,
            version,
            secret_ref,
        } => {
            // Both refusals are decided before a credential is resolved, so a
            // malformed release argument never opens a session. A `smallint`
            // primary key starts at 1; 0 and negatives are not versions.
            if *version < 1 {
                return Err((
                    Exit::PreconditionFailed,
                    format!("a pepper version starts at 1; `{version}` is not one"),
                ));
            }
            if secret_ref.trim().is_empty() {
                return Err((
                    Exit::PreconditionFailed,
                    "a pepper row must name the secret version holding its material".to_owned(),
                ));
            }
            let mut connection = open(cli).await?;
            seed_pepper(&mut connection, pepper.row(), *version, secret_ref)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            let applied = applied_head(&mut connection)
                .await
                .map_err(|error| (runner_exit(&error), error.to_string()))?;
            render(&receipt(applied, applied, "not_requested"))
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli).await {
        Ok(receipt) => {
            println!("{receipt}");
            std::process::ExitCode::from(Exit::Success as u8)
        }
        Err((exit, message)) => {
            eprintln!("central-schema-admin: {message}");
            std::process::ExitCode::from(exit as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser as _;

    use super::{ADVISORY_LOCK_KEY, Cli, Exit, run};
    use central_schema_admin::migration::{MigrationBundle, native_migrator};

    fn bundle_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central")
    }

    fn plan_args() -> Vec<&'static str> {
        vec![
            "central-schema-admin",
            "--database-host",
            "db.example",
            "--database-port",
            "5432",
            "--database-name",
            "aex",
            "--database-secret-arn",
            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:fixture",
            "--tls-root-ca-path",
            "rds-ca.pem",
            "--plane",
            "dev",
            "--release",
            "rel_fixture",
            "plan",
        ]
    }

    #[tokio::test]
    async fn exact_cli_and_exit_contract_are_stable() {
        let cli = Cli::try_parse_from(plan_args()).expect("exact CLI parses");
        let receipt = run(&cli).await.expect("plan is local and credential-free");
        assert!(receipt.contains("\"lockKey\":4703262552200136530"));
        assert_eq!(ADVISORY_LOCK_KEY, 4_703_262_552_200_136_530);
        assert_eq!(Exit::ChecksumDrift as u8, 10);
        assert_eq!(Exit::LockUnavailable as u8, 11);
        assert_eq!(Exit::HeadMismatch as u8, 12);
        assert_eq!(Exit::PreconditionFailed as u8, 13);
        assert_eq!(Exit::GrantDrift as u8, 14);
        assert_eq!(Exit::DestructiveEvidenceMissing as u8, 15);
        assert_eq!(Exit::Connection as u8, 20);
        assert_eq!(Exit::SecretUnavailable as u8, 21);
        assert_eq!(Exit::Conservation as u8, 30);
    }

    #[test]
    fn finance_ddl_has_both_conservation_defences_and_no_bypass() {
        let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
        assert!(ddl.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(ddl.contains("sum(amount_microusd)"));
        assert!(ddl.contains("observed <> declared"));
        assert!(ddl.contains("BEFORE UPDATE OR DELETE"));
        assert!(ddl.contains("balance_microusd <= 0"));
        assert!(!ddl.contains("current_setting"));
        assert!(!ddl.contains("double precision"));
        assert!(!ddl.contains("::float8"));
    }

    #[test]
    fn bundle_is_linear_and_every_header_parses() {
        let path = bundle_path();
        let bundle = MigrationBundle::load(&path).expect("committed bundle is valid");
        let embedded =
            MigrationBundle::embedded().expect("the executable embeds the canonical lock");
        assert_eq!(bundle.head(), 20_260_801_001_200);
        assert_eq!(
            bundle.versions(),
            vec![
                20_260_801_000_000,
                20_260_801_000_100,
                20_260_801_000_200,
                20_260_801_000_300,
                20_260_801_000_400,
                20_260_801_000_500,
                20_260_801_000_600,
                20_260_801_000_700,
                20_260_801_000_800,
                20_260_801_000_900,
                20_260_801_001_000,
                20_260_801_001_100,
                20_260_801_001_200,
            ]
        );
        assert_eq!(embedded.versions(), bundle.versions());
        assert_eq!(embedded.head(), bundle.head());
        assert_eq!(embedded.has_destructive(), bundle.has_destructive());
        assert!(
            !Path::new(&bundle_path())
                .join("approved-checksum-transitions.toml")
                .exists()
        );
    }

    #[tokio::test]
    async fn native_sqlx_migrator_uses_dedicated_history_schema() {
        let migrator = native_migrator();
        let embedded = MigrationBundle::embedded().expect("the embedded lock parses");
        assert_eq!(migrator.table_name, "schema_admin._sqlx_migrations");
        assert_eq!(migrator.create_schemas.as_ref(), ["schema_admin"]);
        assert!(!migrator.ignore_missing);
        assert!(migrator.locking);
        assert_eq!(
            migrator
                .migrations
                .iter()
                .map(|migration| migration.version)
                .collect::<Vec<_>>(),
            embedded.versions(),
            "the SQLx payload and the embedded bundle lock name the same chain"
        );
    }

    #[test]
    fn no_migration_body_carries_a_privilege() {
        // The release gate refuses a `GRANT` or `REVOKE` in a migration body and
        // this asserts the same property from the other side, so the split
        // between "a migration creates objects" and "grants.toml says who may
        // touch them" cannot be undone by a body nobody re-bundled.
        let directory = bundle_path();
        let entries = std::fs::read_dir(&directory).expect("the bundle directory lists");
        let mut checked = 0_usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "sql") {
                continue;
            }
            let body = std::fs::read_to_string(&path).expect("a migration body reads");
            for (number, line) in body.lines().enumerate() {
                let statement = line.trim_start().to_ascii_uppercase();
                assert!(
                    !statement.starts_with("GRANT ") && !statement.starts_with("REVOKE "),
                    "{}:{}: privileges belong in grants.toml",
                    path.display(),
                    number + 1
                );
            }
            checked += 1;
        }
        assert!(checked >= 7, "only {checked} migration bodies were read");
    }
}
