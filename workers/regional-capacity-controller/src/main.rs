//! Regional capacity authority composition root.

use std::process::ExitCode;
use std::sync::Arc;

use aex_capacity_dynamodb::{CapacityStore, canonical_defaults};
use lambda_runtime::{LambdaEvent, service_fn};
use regional_capacity_controller::{CapacityController, Config, SystemClock};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-capacity-controller: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let defaults = match canonical_defaults() {
        Ok(defaults) => defaults,
        Err(error) => {
            eprintln!("regional-capacity-controller: embedded defaults are invalid: {error}");
            return ExitCode::FAILURE;
        }
    };
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let controller = Arc::new(CapacityController::new(
        CapacityStore::new(
            aws_sdk_dynamodb::Client::new(&aws),
            config.authority_table,
            config.projection_table,
        ),
        defaults,
        SystemClock,
    ));
    let outcome = lambda_runtime::run(service_fn(move |event: LambdaEvent<_>| {
        let controller = Arc::clone(&controller);
        async move { controller.handle(event.payload).await }
    }))
    .await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-capacity-controller: runtime stopped: {error}");
            ExitCode::FAILURE
        }
    }
}
