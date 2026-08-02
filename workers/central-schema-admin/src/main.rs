//! One-shot central schema administration task.
//!
//! Every schema fact lives in the library beside this file; the binary is the
//! CLI, the exit contract and the receipt, and nothing else.

use std::path::PathBuf;

use aex_wire::canonical::to_jcs_string;
use central_schema_admin::ADVISORY_LOCK_KEY;
use central_schema_admin::grants::{self, GrantSet};
use central_schema_admin::migration::{self, MigrationBundle};
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

async fn run(cli: &Cli) -> Result<String, (Exit, String)> {
    debug_assert_eq!(Exit::ALL.len(), 10);
    let bundle_path = migration::bundle_path();
    let bundle = MigrationBundle::load(&bundle_path)
        .map_err(|error| (Exit::ChecksumDrift, error.to_string()))?;
    migration::native_migrator()
        .await
        .map_err(|error| (Exit::ChecksumDrift, error.to_string()))?;
    let grants = GrantSet::load(grants::grants_path())
        .map_err(|error| (Exit::GrantDrift, error.to_string()))?;
    grants
        .role("aex_provider_cost")
        .ok_or_else(|| (Exit::GrantDrift, "provider-cost role is absent".to_owned()))?;
    match &cli.command {
        Command::Plan {
            expect_applied_head,
        } => {
            let receipt = Receipt {
                release_id: &cli.release,
                plane: match cli.plane {
                    Plane::Dev => "dev",
                    Plane::Prd => "prd",
                },
                bundle_head: bundle.head(),
                applied_head_before: *expect_applied_head,
                applied_head_after: *expect_applied_head,
                migration_versions: bundle.versions(),
                grant_roles: grants.role_names(),
                lock_key: ADVISORY_LOCK_KEY,
                conservation: "not_requested",
            };
            to_jcs_string(&receipt).map_err(|error| (Exit::ChecksumDrift, error.to_string()))
        }
        Command::Migrate {
            expect_head,
            allow_destructive,
            backup_evidence,
            ..
        } => {
            if *expect_head != bundle.head() {
                return Err((
                    Exit::HeadMismatch,
                    "bundle head does not match --expect-head".into(),
                ));
            }
            if *allow_destructive && backup_evidence.as_deref().is_none_or(str::is_empty) {
                return Err((
                    Exit::DestructiveEvidenceMissing,
                    "destructive migration needs backup evidence".into(),
                ));
            }
            Err((Exit::SecretUnavailable, "credential resolution is deliberately not performed in an uncredentialed rewrite run".into()))
        }
        _ => Err((
            Exit::SecretUnavailable,
            "database operation requires the DDL-only secret resolver".into(),
        )),
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
    use central_schema_admin::migration::{MigrationBundle, bundle_path, native_migrator};

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
        assert_eq!(bundle.head(), 20_260_801_000_700);
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
            ]
        );
        assert!(
            !Path::new(&bundle_path())
                .join("approved-checksum-transitions.toml")
                .exists()
        );
    }

    #[tokio::test]
    async fn native_sqlx_migrator_uses_dedicated_history_schema() {
        let migrator = native_migrator()
            .await
            .expect("SQLx parses committed SQL files");
        assert_eq!(migrator.table_name, "schema_admin._sqlx_migrations");
        assert_eq!(migrator.create_schemas.as_ref(), ["schema_admin"]);
        assert!(!migrator.ignore_missing);
        assert!(migrator.locking);
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
