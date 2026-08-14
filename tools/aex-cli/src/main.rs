//! Thin executable shell over `aex_cli`.

use std::io::{self, Write as _};
use std::process::ExitCode;

use aex_cli::{Cli, command_registry, marked_command};
use clap::FromArgMatches as _;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "error: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run() -> Result<(), aex_cli::runtime::RuntimeError> {
    let matches = marked_command().get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(|error| {
        // Clap normally exits before this boundary; this branch protects only
        // programmatic argument sources and contains no credential value.
        aex_cli::runtime::parse_error(error.to_string())
    })?;
    if cli.dump_command_registry {
        serde_json::to_writer(io::stdout(), &command_registry())
            .map_err(|_| aex_cli::runtime::output_error())?;
        writeln!(io::stdout()).map_err(|_| aex_cli::runtime::output_error())?;
        return Ok(());
    }
    aex_cli::runtime::execute(cli).await
}
