//! `aex-release-tool` command-line entry point.

use clap::Parser;

/// the release graph, admission, manifest and evidence binary used by CI and deploy.
#[derive(Debug, Parser)]
#[command(
    name = "aex-release-tool",
    version,
    about = "the release graph, admission, manifest and evidence binary used by CI and deploy"
)]
struct Cli {
    /// Print the resolved invocation instead of executing it.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::bail!(
        "`aex-release-tool` has no implementation yet (dry_run = {})",
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
