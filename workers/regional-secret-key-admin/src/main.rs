//! `regional-secret-key-admin` composition root (one-shot Rust artifact).
//!
//! Exclusive responsibility: hierarchical branch-key creation, rotation and
//! verification under the privileged KMS role. No server, no queue, no schedule.
//!
//! Exit codes are the interface (RS-16): `0` acted, `3` was already current and
//! mutated nothing, `1` refused or failed. The release pipeline distinguishes
//! "created" from "already current" without parsing output.

use std::process::ExitCode;

use aex_regional_http::config::RegionalHttpConfigError;
use aex_wire::ids::{PrefixedId as _, WorkspaceId};
use clap::{Parser, Subcommand};
use regional_secret_key_admin::AdminOutcome;
use regional_secret_key_admin::admin::{AdminCommand, AdminRunError, execute};
use regional_secret_key_admin::aws::AwsKeyAdmin;
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
}

/// Why the task refused or failed.
#[derive(Debug, thiserror::Error)]
enum AdminError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// The workspace argument was not a `wsp_` identifier.
    #[error("`{0}` is not a `wsp_` workspace identifier")]
    Workspace(String),
    /// The real provider/store administration path refused or failed.
    #[error(transparent)]
    Admin(#[from] AdminRunError),
}

/// The exit code that means "already current; nothing was mutated".
const ALREADY_CURRENT: u8 = 3;
const DEPLOYABLE: &str = "regional-secret-key-admin";

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return ExitCode::FAILURE;
    }
    let cli = Cli::parse();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE,
                error = %error,
                "process configuration rejected"
            );
            eprintln!("{DEPLOYABLE}: refusing to run: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = run(&cli, &config).await;
    match outcome {
        Ok(AdminOutcome::Created | AdminOutcome::Rotated) => ExitCode::SUCCESS,
        Ok(AdminOutcome::AlreadyCurrent) => ExitCode::from(ALREADY_CURRENT),
        Err(error) => {
            eprintln!("{DEPLOYABLE}: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli, config: &Config) -> Result<AdminOutcome, AdminError> {
    let workspace = WorkspaceId::parse(cli.command.workspace())
        .map_err(|_| AdminError::Workspace(cli.command.workspace().to_owned()))?;
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE,
        plane = config.plane.as_str(),
        region = config.region.as_str(),
        "process started"
    );

    // Startup admission has already refused a wrong keystore, a cross-region key
    // and every product-table binding. The keystore binding below is the last
    // structural gate before any AWS call: it names the table and the key
    // together so a mismatched pair cannot be administered.
    let binding = aex_secret_keystore_dynamodb::store::KeyStoreBinding::new(
        config.keystore_table.clone(),
        config.secret_kms_key.value.clone(),
    );
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let admin = AwsKeyAdmin::new(
        aws_sdk_dynamodb::Client::new(&aws),
        aws_sdk_kms::Client::new(&aws),
        binding.table(),
        binding.kms_key_arn(),
        config.plane.as_str(),
        config.region.as_str(),
    );
    let command = match &cli.command {
        Command::CreateBranchKey { .. } => AdminCommand::Create,
        Command::RotateBranchKey { reason, .. } => AdminCommand::Rotate { reason },
        Command::Verify { .. } => AdminCommand::Verify,
    };
    let outcome = execute(&admin, command, workspace, config.attestation).await?;
    println!(
        "regional-secret-key-admin: outcome={outcome:?} workspace={workspace} keystore={} attestation={}",
        binding.table(),
        config.attestation
    );
    Ok(outcome)
}
