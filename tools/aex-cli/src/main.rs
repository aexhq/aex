//! Thin executable shell over `aex_cli`.

use std::io::{self, Write};
use std::process::ExitCode;

use aex_cli::{Cli, Command, command_registry, marked_command, render_completions};
use clap::FromArgMatches as _;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    // Parsed through the marked tree rather than `Cli::parse`, so `aex --help`
    // and `aex <group> --help` render the deferral the contract declares.
    let matches = marked_command().get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(|error| error.to_string())?;
    if cli.dump_command_registry {
        serde_json::to_writer(io::stdout(), &command_registry())
            .map_err(|error| error.to_string())?;
        writeln!(io::stdout()).map_err(|error| error.to_string())?;
        return Ok(());
    }
    match cli.command {
        Some(Command::Completions { shell }) => {
            let name = format!("{shell:?}").to_ascii_lowercase();
            io::stdout()
                .write_all(&render_completions(&name)?)
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        Some(Command::Version) => {
            println!(
                "aex {} target={} contract=fec7f531dec7",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH
            );
            Ok(())
        }
        Some(_) => Err("network command execution is not composed in this build".to_owned()),
        None => Err("a command is required".to_owned()),
    }
}
