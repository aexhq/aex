//! `central-control-worker` composition root (Rust Lambda ZIP).
//!
//! The composition lives in the library beside this file. The binary is the
//! process: diagnostics installation, the start-up refusal and the exit code, and
//! nothing else.

use central_control_worker::{Config, DEPLOYABLE, run};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("central-control-worker: refusing to start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE.as_str(),
                error = %error,
                "configuration rejected"
            );
            eprintln!("central-control-worker: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = run(&config).await;
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-worker: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
