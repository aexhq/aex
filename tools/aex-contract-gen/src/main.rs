//! `aex-contract-gen` command-line entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// The deterministic contract generator that produces `api/generated/` and the
/// generated surface of `aex-wire`.
#[derive(Debug, Parser)]
#[command(
    name = "aex-contract-gen",
    version,
    about = "the deterministic AEX contract generator"
)]
struct Cli {
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// The four verbs.
#[derive(Debug, Subcommand)]
enum Command {
    /// Write every generated output.
    Build {
        /// Repository root; defaults to the workspace this binary was built in.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Regenerate into memory and compare against the committed output.
    Check {
        /// Repository root; defaults to the workspace this binary was built in.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Print the contract digest.
    Digest {
        /// Repository root; defaults to the workspace this binary was built in.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Classify a head bundle against a base bundle.
    Classify {
        /// Path to the base `bundle.json`.
        #[arg(long)]
        base: PathBuf,
        /// Path to the head `bundle.json`.
        #[arg(long)]
        head: PathBuf,
        /// `json` or `text`.
        #[arg(long, default_value = "text")]
        format: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build { root } => {
            let root = root.unwrap_or_else(aex_contract_gen::load::repo_root);
            let tree = aex_contract_gen::build(&root)?;
            println!("aex-contract-gen: wrote {} file(s)", tree.len());
            Ok(())
        }
        Command::Check { root } => {
            let root = root.unwrap_or_else(aex_contract_gen::load::repo_root);
            let drift = aex_contract_gen::check(&root)?;
            if drift.is_empty() {
                println!("aex-contract-gen: generated output is up to date");
                return Ok(());
            }
            for line in &drift {
                eprintln!("  {line}");
            }
            anyhow::bail!(
                "{} generated file(s) drifted; run `cargo run -p aex-contract-gen -- build`",
                drift.len()
            )
        }
        Command::Digest { root } => {
            let root = root.unwrap_or_else(aex_contract_gen::load::repo_root);
            let ir = aex_contract_gen::load::load(&root)?;
            println!("{}", aex_contract_gen::emit::contract_digest(&ir));
            Ok(())
        }
        Command::Classify { base, head, format } => {
            let base: serde_json::Value = serde_json::from_slice(&std::fs::read(&base)?)?;
            let head: serde_json::Value = serde_json::from_slice(&std::fs::read(&head)?)?;
            let changes = aex_contract_gen::classify::classify(&base, &head);
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&changes)?);
            } else {
                for change in &changes {
                    println!(
                        "{:?}\t{:?}\t{}",
                        change.classification, change.subject, change.detail
                    );
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
