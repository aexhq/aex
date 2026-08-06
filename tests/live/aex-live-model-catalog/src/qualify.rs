//! Protected exact-target model-catalog qualifier.

use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use aex_live_model_catalog::ProviderKeys;
use aex_live_model_catalog::deepseek_qualification::{
    DEEPSEEK_FLASH_MODEL, NegativeInputs, ProtectedCredential, execute_matrix,
    maximum_matrix_cost_micro_usd, unsupported_required_probes,
};
use aex_live_model_catalog::genesis::deepseek_v4_flash_candidate;
use aex_live_model_catalog::qualification_output::{
    QUALIFICATION_DIRECTORY, QualificationMetadata, QualificationOutputError, QualificationPlane,
    QualificationRegion, QualificationRepository, ReviewedSourceSha, build_qualification_outputs,
    write_qualification_outputs,
};
use aex_live_model_catalog::tokenizer_oracle::{DeepSeekTokenizerOracle, TokenizerOracleError};
use aex_model_catalog::document::PublisherId;
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::receipt::ProbeId;
use aex_wire::types::Timestamp;
use clap::{Parser, Subcommand};

const AUTHORITATIVE_CREDENTIAL: &str = "AEX_LIVE_PROVIDER_KEY_DEEPSEEK";
const MAXIMUM_OPERATOR_BUDGET_MICRO_USD: u64 = 5_000_000;
const MAXIMUM_RUNTIME_SECONDS: u32 = 3_600;

#[derive(Debug, Parser)]
#[command(name = "aex-model-catalog-qualifier")]
#[command(about = "fail-closed protected DeepSeek model qualification")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the exact non-secret target/program readiness projection.
    Preflight,
    /// Run the exact matrix and emit outputs only after every required pass.
    Run {
        /// Exact reviewed source commit which runs the live matrix.
        #[arg(long)]
        source_sha: String,
        /// GitHub Actions numeric run id.
        #[arg(long)]
        run_id: String,
        /// GitHub Actions run attempt, starting at one.
        #[arg(long)]
        run_attempt: u32,
        /// Owner-approved maximum estimated provider spend.
        #[arg(long)]
        maximum_budget_micro_usd: u64,
        /// Whole qualification deadline.
        #[arg(long)]
        maximum_runtime_seconds: u32,
        /// Verified official `tokenizer.json`; its exact SHA-256 is enforced.
        #[arg(long)]
        tokenizer_json: PathBuf,
        /// Fixed all-or-none output directory. Never written on a failed run.
        #[arg(long)]
        output_dir: PathBuf,
    },
}

struct RunRequest {
    source_sha: String,
    run_id: String,
    run_attempt: u32,
    maximum_budget_micro_usd: u64,
    maximum_runtime_seconds: u32,
    tokenizer_json: PathBuf,
    output_dir: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("model-catalog-qualifier: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), QualifierError> {
    match cli.command {
        Command::Preflight => preflight()?,
        Command::Run {
            source_sha,
            run_id,
            run_attempt,
            maximum_budget_micro_usd,
            maximum_runtime_seconds,
            tokenizer_json,
            output_dir,
        } => {
            qualify(RunRequest {
                source_sha,
                run_id,
                run_attempt,
                maximum_budget_micro_usd,
                maximum_runtime_seconds,
                tokenizer_json,
                output_dir,
            })
            .await?;
        }
    }
    Ok(())
}

fn preflight() -> Result<(), QualifierError> {
    let unsupported = unsupported_required_probes();
    let candidate = deepseek_v4_flash_candidate()?;
    let maximum_cost_micro_usd = maximum_matrix_cost_micro_usd(&candidate)?;
    println!(
        "{}",
        serde_json::json!({
            "schema": "aex.model-catalog-qualification-preflight.v1",
            "provider": "deepseek",
            "model": DEEPSEEK_FLASH_MODEL,
            "programs": ProbeId::ALL.len(),
            "unsupportedRequired": unsupported,
            "ready": unsupported.is_empty(),
            "credentialVariable": AUTHORITATIVE_CREDENTIAL,
            "maximumBudgetMicroUsdCeiling": MAXIMUM_OPERATOR_BUDGET_MICRO_USD,
            "maximumRuntimeSecondsCeiling": MAXIMUM_RUNTIME_SECONDS,
            "maximumMatrixCostMicroUsd": maximum_cost_micro_usd,
        })
    );
    Ok(())
}

async fn qualify(request: RunRequest) -> Result<(), QualifierError> {
    validate_bounds(
        request.maximum_budget_micro_usd,
        request.maximum_runtime_seconds,
    )?;
    validate_output_directory(&request.output_dir)?;
    let parsed_run_id = request
        .run_id
        .parse::<u64>()
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or(QualifierError::InvalidRunId)?;
    let parsed_run_attempt =
        NonZeroU32::new(request.run_attempt).ok_or(QualifierError::InvalidRunAttempt)?;
    let approved_budget = NonZeroU64::new(request.maximum_budget_micro_usd)
        .ok_or(QualifierError::BudgetOutOfRange)?;
    let approved_runtime = NonZeroU32::new(request.maximum_runtime_seconds)
        .ok_or(QualifierError::RuntimeOutOfRange)?;
    let reviewed_source = ReviewedSourceSha::new(&request.source_sha)?;
    let missing = unsupported_required_probes();
    if !missing.is_empty() {
        return Err(QualifierError::IncompletePrograms { probes: missing });
    }
    let negative = NegativeInputs::for_run(&request.run_id, request.run_attempt)?;
    let candidate = deepseek_v4_flash_candidate()?;
    let tokenizer_bytes =
        std::fs::read(request.tokenizer_json).map_err(|_| QualifierError::TokenizerRead)?;
    let oracle = DeepSeekTokenizerOracle::from_pinned_json(&tokenizer_bytes)?;
    let corpora = oracle.build_corpora(candidate.limits.context_window_tokens)?;
    let admitted_cost = maximum_matrix_cost_micro_usd(&candidate)?;
    if admitted_cost > request.maximum_budget_micro_usd {
        return Err(QualifierError::BudgetInsufficient {
            required_micro_usd: admitted_cost,
            approved_micro_usd: request.maximum_budget_micro_usd,
        });
    }
    let ran_at = now_timestamp()?;
    let (variable, plaintext) =
        ProviderKeys::require_named(aex_wire::provider::ProviderId::Deepseek);
    if variable != AUTHORITATIVE_CREDENTIAL {
        return Err(QualifierError::LegacyCredentialVariable);
    }
    let credential = ProtectedCredential::new(plaintext)?;
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(u64::from(request.maximum_runtime_seconds)),
        execute_matrix(
            variable,
            candidate,
            credential,
            negative,
            corpora,
            request.maximum_budget_micro_usd,
        ),
    )
    .await
    .map_err(|_| QualifierError::Deadline)??;
    if report.maximum_cost_micro_usd != admitted_cost {
        return Err(QualifierError::MaximumCostChanged);
    }
    if report.runs.iter().any(|run| {
        run.probe
            .is_required_for(aex_live_model_catalog::deepseek_qualification::GENESIS_CAPABILITIES)
            && !run.outcome.is_pass()
    }) {
        return Err(QualifierError::RequiredProbeFailed);
    }
    let issued_at = now_timestamp()?;
    let outputs = build_qualification_outputs(
        report.runs,
        report.tokens_spent,
        report.maximum_cost_micro_usd,
        &QualificationMetadata {
            repository: QualificationRepository::AexhqAex,
            source_sha: reviewed_source,
            run_id: parsed_run_id,
            run_attempt: parsed_run_attempt,
            ran_at,
            issued_at,
            plane: QualificationPlane::Dev,
            region: QualificationRegion::EuWest1,
            publisher: PublisherId(BoundedString::truncating("aex-catalog-prd")),
            approved_budget_micro_usd: approved_budget,
            approved_runtime_seconds: approved_runtime,
        },
    )?;
    write_qualification_outputs(Path::new("."), &outputs)?;
    Ok(())
}

fn validate_bounds(budget: u64, runtime: u32) -> Result<(), QualifierError> {
    if budget == 0 || budget > MAXIMUM_OPERATOR_BUDGET_MICRO_USD {
        return Err(QualifierError::BudgetOutOfRange);
    }
    if runtime == 0 || runtime > MAXIMUM_RUNTIME_SECONDS {
        return Err(QualifierError::RuntimeOutOfRange);
    }
    Ok(())
}

fn validate_output_directory(path: &Path) -> Result<(), QualifierError> {
    if path != Path::new(QUALIFICATION_DIRECTORY) || std::fs::symlink_metadata(path).is_ok() {
        return Err(QualifierError::UnsafeOutputPath);
    }
    Ok(())
}

fn now_timestamp() -> Result<Timestamp, QualifierError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| QualifierError::Clock)?;
    let millis = i64::try_from(elapsed.as_millis()).map_err(|_| QualifierError::Clock)?;
    Timestamp::from_unix_millis(millis).map_err(|_| QualifierError::Clock)
}

#[derive(Debug, thiserror::Error)]
enum QualifierError {
    #[error("required qualification programs are not implemented: {probes:?}")]
    IncompletePrograms { probes: Vec<ProbeId> },
    #[error("maximum_budget_micro_usd is outside the protected ceiling")]
    BudgetOutOfRange,
    #[error("maximum_runtime_seconds is outside the protected ceiling")]
    RuntimeOutOfRange,
    #[error("run_id must be a non-zero base-10 u64")]
    InvalidRunId,
    #[error("run_attempt must be non-zero")]
    InvalidRunAttempt,
    #[error(
        "qualification output must be the absent fixed .tmp/model-catalog/qualification directory"
    )]
    UnsafeOutputPath,
    #[error(
        "maximum matrix cost {required_micro_usd} exceeds approved budget {approved_micro_usd}"
    )]
    BudgetInsufficient {
        required_micro_usd: u64,
        approved_micro_usd: u64,
    },
    #[error("maximum matrix cost changed after provider execution")]
    MaximumCostChanged,
    #[error("the legacy DeepSeek credential alias is forbidden in the protected workflow")]
    LegacyCredentialVariable,
    #[error("the pinned tokenizer JSON could not be read")]
    TokenizerRead,
    #[error(transparent)]
    Tokenizer(#[from] TokenizerOracleError),
    #[error("the whole qualification deadline elapsed")]
    Deadline,
    #[error("one or more required qualification probes failed")]
    RequiredProbeFailed,
    #[error("the system clock cannot produce a canonical qualification timestamp")]
    Clock,
    #[error(transparent)]
    Input(#[from] aex_live_model_catalog::deepseek_qualification::QualificationError),
    #[error(transparent)]
    Run(#[from] aex_live_model_catalog::deepseek_qualification::QualificationRunError),
    #[error(transparent)]
    Genesis(#[from] aex_live_model_catalog::genesis::GenesisError),
    #[error(transparent)]
    Output(#[from] QualificationOutputError),
}
