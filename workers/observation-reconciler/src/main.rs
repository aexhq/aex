//! `observation-reconciler` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: one control duty of the observation authority,
//! selected by `AEX_OBS_DUTY` and bounded by the page, shard and attempt limits
//! the deployment declares.
//!
//! This binary is a composition root only. Configuration is validated before
//! anything starts, telemetry is installed through `aex_platform_telemetry`, the
//! capability grant is asserted against the role the configured duty selects,
//! and the behaviour lives in [`observation_reconciler`].

use observation_reconciler::config::Config;
use observation_reconciler::{refusal, run};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", refusal(&error));
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );
    let outcome = run(config).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("observation-reconciler: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("observation-reconciler: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
