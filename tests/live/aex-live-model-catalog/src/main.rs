//! Protected model-catalog publisher command.

use std::path::PathBuf;
use std::process::ExitCode;

use aex_live_model_catalog::catalog_source::{generate_static_catalog, validate_static_catalog};
use aex_live_model_catalog::publisher::{
    KmsPublicKeyOutput, KmsSignOutput, SignatureRecord, SigningRequest, assemble, canonical_json,
    prepare_signing_request, record_kms_signature, trust_roots_from_kms,
};
use aex_wire::types::Timestamp;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "aex-model-catalog-publisher")]
#[command(about = "fail-closed protected model-catalog publication")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Render and preflight a canonical document from reviewed static source.
    Generate {
        /// Exact canonical source envelope checked into the repository.
        #[arg(long)]
        source: PathBuf,
        /// Explicit validation instant in Unix milliseconds.
        #[arg(long)]
        now_ms: i64,
        /// Exact canonical catalog document output.
        #[arg(long)]
        out: PathBuf,
    },
    /// Recompute and independently preflight one generated document.
    Validate {
        /// Exact canonical source envelope checked into the repository.
        #[arg(long)]
        source: PathBuf,
        /// Exact canonical catalog document to validate.
        #[arg(long)]
        document: PathBuf,
        /// Explicit validation instant in Unix milliseconds.
        #[arg(long)]
        now_ms: i64,
    },
    /// Validate a KMS P-256 public key and emit canonical runtime trust roots.
    TrustRoots {
        /// Closed JSON projection emitted by `aws kms get-public-key`.
        #[arg(long)]
        kms_output: PathBuf,
        /// Exact KMS key ARN requested by the protected workflow.
        #[arg(long)]
        expected_kms_key_arn: String,
        /// Stable logical id compiled into runtime trust roots.
        #[arg(long)]
        key_id: String,
        /// Canonical runtime trust-root JSON output.
        #[arg(long)]
        out: PathBuf,
    },
    /// Validate a canonical document and emit its KMS DIGEST signing request.
    Prepare {
        /// Exact canonical catalog document.
        #[arg(long)]
        document: PathBuf,
        /// Explicit validation instant in Unix milliseconds.
        #[arg(long)]
        now_ms: i64,
        /// Canonical signing request output.
        #[arg(long)]
        out: PathBuf,
    },
    /// Bind an AWS KMS Sign response to a prepared request.
    RecordSignature {
        /// Canonical signing request from `prepare`.
        #[arg(long)]
        request: PathBuf,
        /// Stable logical id used in runtime trust roots.
        #[arg(long)]
        key_id: String,
        /// Exact KMS key ARN that must have produced the signature.
        #[arg(long)]
        expected_kms_key_arn: String,
        /// Exact JSON emitted by `aws kms sign`.
        #[arg(long)]
        kms_output: PathBuf,
        /// Canonical signature-record output.
        #[arg(long)]
        out: PathBuf,
    },
    /// Assemble and verify a genesis or retain-all chain extension.
    Assemble {
        /// Exact canonical catalog document.
        #[arg(long)]
        document: PathBuf,
        /// Canonical detached signature record.
        #[arg(long)]
        signature: PathBuf,
        /// Canonical runtime trust-root JSON.
        #[arg(long)]
        trust_roots: PathBuf,
        /// Previously published canonical collection; omit only for genesis.
        #[arg(long)]
        previous_collection: Option<PathBuf>,
        /// Explicit validation instant in Unix milliseconds.
        #[arg(long)]
        now_ms: i64,
        /// Canonical signed collection output.
        #[arg(long)]
        out: PathBuf,
        /// Canonical release-binding identities output.
        #[arg(long)]
        binding_out: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("model-catalog-publisher: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Generate {
            source,
            now_ms,
            out,
        } => {
            let source = std::fs::read(source)?;
            let generated = generate_static_catalog(&source, timestamp(now_ms)?)?;
            std::fs::write(out, generated.canonical_document)?;
        }
        Command::Validate {
            source,
            document,
            now_ms,
        } => {
            let source = std::fs::read(source)?;
            let document = std::fs::read(document)?;
            validate_static_catalog(&source, &document, timestamp(now_ms)?)?;
        }
        Command::TrustRoots {
            kms_output,
            expected_kms_key_arn,
            key_id,
            out,
        } => {
            let kms: KmsPublicKeyOutput = serde_json::from_slice(&std::fs::read(kms_output)?)?;
            let roots = trust_roots_from_kms(&kms, &expected_kms_key_arn, &key_id)?;
            std::fs::write(out, roots)?;
        }
        Command::Prepare {
            document,
            now_ms,
            out,
        } => {
            let document = std::fs::read(document)?;
            let request = prepare_signing_request(&document, timestamp(now_ms)?)?;
            std::fs::write(out, canonical_json(&request)?)?;
        }
        Command::RecordSignature {
            request,
            key_id,
            expected_kms_key_arn,
            kms_output,
            out,
        } => {
            let request: SigningRequest = serde_json::from_slice(&std::fs::read(request)?)?;
            let kms: KmsSignOutput = serde_json::from_slice(&std::fs::read(kms_output)?)?;
            let record = record_kms_signature(&request, &key_id, &expected_kms_key_arn, kms)?;
            std::fs::write(out, canonical_json(&record)?)?;
        }
        Command::Assemble {
            document,
            signature,
            trust_roots,
            previous_collection,
            now_ms,
            out,
            binding_out,
        } => {
            let document = std::fs::read(document)?;
            let signature: SignatureRecord = serde_json::from_slice(&std::fs::read(signature)?)?;
            let roots = std::fs::read(trust_roots)?;
            let previous = previous_collection.map(std::fs::read).transpose()?;
            let publication = assemble(
                &document,
                &signature,
                &roots,
                timestamp(now_ms)?,
                previous.as_deref(),
            )?;
            std::fs::write(out, publication.collection)?;
            std::fs::write(binding_out, canonical_json(&publication.binding)?)?;
        }
    }
    Ok(())
}

fn timestamp(millis: i64) -> Result<Timestamp, aex_wire::types::ValueError> {
    Timestamp::from_unix_millis(millis)
}
