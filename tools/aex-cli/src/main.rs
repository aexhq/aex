//! `aex-cli` command-line entry point.

use clap::Parser;

/// the native public CLI over the generated `aex-wire` client.
#[derive(Debug, Parser)]
#[command(
    name = "aex-cli",
    version,
    about = "the native public CLI over the generated `aex-wire` client"
)]
struct Cli {
    /// Print the resolved invocation instead of executing it.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::bail!(
        "`aex-cli` has no implementation yet (dry_run = {})",
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
