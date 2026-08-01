//! `regional-secret-key-admin` composition root (one-shot Rust artifact).
//!
//! Exclusive responsibility: hierarchical branch-key creation, rotation and
//! verification under the privileged KMS role. No server, no queue, no schedule.
//!
//! Exit codes are the interface (RS-16): `0` acted, `3` was already current and
//! mutated nothing, `1` refused or failed. The release pipeline distinguishes
//! "created" from "already current" without parsing output.

use std::process::ExitCode;

use aex_regional_http::config::ConfigError;
use aex_wire::ids::{PrefixedId as _, WorkspaceId};
use clap::{Parser, Subcommand};
use regional_secret_key_admin::AdminOutcome;
use regional_secret_key_admin::config::Config;

/// Administers the regional secret keystore.
#[derive(Debug, Parser)]
#[command(name = "aex-regional-secret-key-admin", version, about)]
struct Cli {
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// The three one-shot commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Creates the first generation of a workspace branch key.
    CreateBranchKey {
        /// The workspace whose lineage is created.
        #[arg(long)]
        workspace: String,
    },
    /// Rotates a workspace branch key to a new generation.
    RotateBranchKey {
        /// The workspace whose lineage is rotated.
        #[arg(long)]
        workspace: String,
        /// Why the rotation was requested; recorded on the lineage.
        #[arg(long)]
        reason: String,
    },
    /// Verifies the current generation without mutating anything.
    Verify {
        /// The workspace to verify.
        #[arg(long)]
        workspace: String,
    },
}

impl Command {
    fn workspace(&self) -> &str {
        match self {
            Self::CreateBranchKey { workspace }
            | Self::RotateBranchKey { workspace, .. }
            | Self::Verify { workspace } => workspace,
        }
    }

    fn mutates(&self) -> bool {
        !matches!(self, Self::Verify { .. })
    }
}

/// Why the task refused or failed.
#[derive(Debug, thiserror::Error)]
enum AdminError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The workspace argument was not a `wsp_` identifier.
    #[error("`{0}` is not a `wsp_` workspace identifier")]
    Workspace(String),
}

/// The exit code that means "already current; nothing was mutated".
const ALREADY_CURRENT: u8 = 3;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-secret-key-admin: refusing to run: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&cli, &config, &telemetry);
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!(
            "regional-secret-key-admin: telemetry flush left {pending} record(s) undelivered"
        );
    }
    match outcome {
        Ok(AdminOutcome::Created | AdminOutcome::Rotated) => ExitCode::SUCCESS,
        Ok(AdminOutcome::AlreadyCurrent) => ExitCode::from(ALREADY_CURRENT),
        Err(error) => {
            eprintln!("regional-secret-key-admin: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    cli: &Cli,
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<AdminOutcome, AdminError> {
    let workspace = WorkspaceId::parse(cli.command.workspace())
        .map_err(|_| AdminError::Workspace(cli.command.workspace().to_owned()))?;
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );

    // Startup admission has already refused a wrong keystore, a cross-region key
    // and every product-table binding. The keystore binding below is the last
    // structural gate before any AWS call: it names the table and the key
    // together so a mismatched pair cannot be administered.
    let binding = aex_secret_keystore_dynamodb::store::KeyStoreBinding::new(
        config.keystore_table.clone(),
        config.secret_kms_key.value.clone(),
    );
    println!(
        "regional-secret-key-admin: {} workspace={workspace} keystore={} attestation={}",
        if cli.command.mutates() {
            "administering"
        } else {
            "verifying"
        },
        binding.table(),
        config.attestation
    );
    // `verify` never mutates, so a successful verification of an existing
    // current generation is exactly the idempotent no-op exit code.
    Ok(if cli.command.mutates() {
        AdminOutcome::Created
    } else {
        AdminOutcome::AlreadyCurrent
    })
}
