//! `central-control-worker` composition root (Rust Lambda ZIP).
//!
//! The composition lives in the library beside this file. The binary is the
//! process: the start-up refusal, the telemetry flush and the exit code, and
//! nothing else.

use central_control_worker::{Config, DEPLOYABLE, run};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr.
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.emit(
                aex_platform_telemetry::Record::event(
                    aex_telemetry_schema::generated::EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
                )
                .with(
                    aex_telemetry_schema::generated::AEX_DEPLOYABLE,
                    DEPLOYABLE.as_str(),
                ),
            );
            let _ = telemetry.flush(settings.flush_deadline);
            eprintln!("central-control-worker: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("central-control-worker: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-worker: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
