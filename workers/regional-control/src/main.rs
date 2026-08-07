//! Direct-invoke regional workspace control authority.

use std::process::ExitCode;
use std::sync::Arc;

use aex_internal_contracts::control::{RegionalControlEnvelope, RegionalControlRequest};
use aex_wire::types::Region;
use lambda_runtime::{LambdaEvent, service_fn};

mod keys {
    pub const REGION: &str = "AEX_REGION";
    pub const SESSION_TABLE: &str = "AEX_SESSION_TABLE";
    pub const ALL: &[&str] = &[REGION, SESSION_TABLE];
}

/// Why this authority refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum RegionalControlConfigError {
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid { name: &'static str, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    region: Region,
    session_table: String,
}

impl Config {
    fn from_env() -> Result<Self, RegionalControlConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup<F>(lookup: F) -> Result<Self, RegionalControlConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let region_raw = required(&lookup, keys::REGION)?;
        let region = Region::from_name(&region_raw).ok_or_else(|| RegionalControlConfigError::Invalid {
            name: keys::REGION,
            reason: format!("`{region_raw}` is not a launch region"),
        })?;
        Ok(Self {
            region,
            session_table: required(&lookup, keys::SESSION_TABLE)?,
        })
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, RegionalControlConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(RegionalControlConfigError::Missing(name)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-control: refusing to start: {error}");
            eprintln!(
                "regional-control: required configuration: {}",
                keys::ALL.join(", ")
            );
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let store = Arc::new(
        aex_session_dynamodb::regional_control::RegionalWorkspaceStore::new(
            aws_sdk_dynamodb::Client::new(&aws),
            config.session_table,
            config.region,
        ),
    );
    if let Err(error) = store.probe().await {
        eprintln!("regional-control: session authority refused readiness: {error}");
        return ExitCode::FAILURE;
    }
    let outcome = lambda_runtime::run(service_fn(
        move |event: LambdaEvent<RegionalControlEnvelope<RegionalControlRequest>>| {
            let store = Arc::clone(&store);
            async move { Ok::<_, std::convert::Infallible>(store.handle(event.payload).await) }
        },
    ))
    .await;
    let _ = telemetry.flush(settings.flush_deadline);
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-control: runtime stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, RegionalControlConfigError, keys};
    use aex_wire::types::Region;
    use std::collections::BTreeMap;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (keys::REGION, "eu-west-1".to_owned()),
            (keys::SESSION_TABLE, "aex-dev-session-authority".to_owned()),
        ])
    }

    #[test]
    fn configuration_binds_one_launch_region_and_table() {
        let vars = complete();
        let config = Config::from_lookup(|name| vars.get(name).cloned()).expect("valid");
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(config.session_table, "aex-dev-session-authority");
    }

    #[test]
    fn every_input_is_required_and_an_unknown_region_is_refused() {
        for key in keys::ALL {
            let mut vars = complete();
            vars.remove(key);
            assert_eq!(
                Config::from_lookup(|name| vars.get(name).cloned()),
                Err(RegionalControlConfigError::Missing(key))
            );
        }
        let mut vars = complete();
        vars.insert(keys::REGION, "mars-central-1".to_owned());
        assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_err());
    }
}
