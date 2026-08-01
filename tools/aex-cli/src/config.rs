#![allow(
    missing_docs,
    reason = "the public resolution behavior is documented on its entry point"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::output::OutputFormat;

/// One non-secret CLI profile.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Profile {
    pub central_url: Option<String>,
    pub regional_url: Option<String>,
    pub workspace: Option<String>,
    pub output: Option<OutputFormat>,
}

/// Injectable resolution inputs.
#[derive(Clone, Debug)]
pub struct ConfigInputs {
    pub central_url: Option<String>,
    pub output: Option<OutputFormat>,
    pub profile: Profile,
    pub env: BTreeMap<String, String>,
    pub stdout_is_terminal: bool,
}

/// Fully resolved local policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub central_url: String,
    pub output: OutputFormat,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid AEX_OUTPUT value {0}")]
    InvalidOutput(String),
    #[error("central URL must be HTTPS")]
    InsecureCentralUrl,
}

/// Apply flag → environment → profile → built-in precedence per setting.
///
/// # Errors
///
/// Returns an error for an unknown output format or a non-HTTPS central URL.
pub fn resolve_config(input: ConfigInputs) -> Result<ResolvedConfig, ConfigError> {
    let central_url = input
        .central_url
        .or_else(|| input.env.get("AEX_CENTRAL_URL").cloned())
        .or(input.profile.central_url)
        .unwrap_or_else(|| "https://api.aex.dev".to_owned());
    if !central_url.starts_with("https://") {
        return Err(ConfigError::InsecureCentralUrl);
    }
    let output = if let Some(output) = input.output {
        output
    } else if let Some(value) = input.env.get("AEX_OUTPUT") {
        value
            .parse()
            .map_err(|()| ConfigError::InvalidOutput(value.clone()))?
    } else {
        input.profile.output.unwrap_or(if input.stdout_is_terminal {
            OutputFormat::Text
        } else {
            OutputFormat::Json
        })
    };
    Ok(ResolvedConfig {
        central_url,
        output,
    })
}

/// Unix credential files must be readable only by their owner.
#[must_use]
pub const fn credentials_mode_is_private(mode: u32) -> bool {
    mode.trailing_zeros() >= 6
}
