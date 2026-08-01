//! `aex-contract-gen` command-line entry point.

use clap::Parser;

/// the deterministic contract generator that produces `api/generated/` and `aex-wire`.
#[derive(Debug, Parser)]
#[command(
    name = "aex-contract-gen",
    version,
    about = "the deterministic contract generator that produces `api/generated/` and `aex-wire`"
)]
struct Cli {
    /// Print the resolved invocation instead of executing it.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::bail!(
        "`aex-contract-gen` has no implementation yet (dry_run = {})",
        cli.dry_run
    );
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
