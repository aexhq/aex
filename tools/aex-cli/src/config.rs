#![allow(
    missing_docs,
    reason = "the public resolution behavior is documented on its entry points"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::output::OutputFormat;

/// One non-secret CLI profile. Secret values are deliberately impossible to
/// represent: `api_key_ref` stores only an environment-variable or protected
/// file reference.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Profile {
    pub central_url: Option<String>,
    pub regional_url: Option<String>,
    pub api_key_ref: Option<String>,
    pub dashboard_session_ref: Option<String>,
    pub output: Option<OutputFormat>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigFile {
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// Injectable resolution inputs.
#[derive(Clone, Debug)]
pub struct ConfigInputs {
    pub central_url: Option<String>,
    pub regional_url: Option<String>,
    pub output: Option<OutputFormat>,
    pub profile: Profile,
    pub env: BTreeMap<String, String>,
    pub stdout_is_terminal: bool,
}

/// Fully resolved non-secret local policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub central_url: String,
    pub regional_url: String,
    pub output: OutputFormat,
}

#[derive(Debug, Error)]
pub enum CliConfigError {
    #[error("invalid AEX_OUTPUT value {0}")]
    InvalidOutput(String),
    #[error("central and regional URLs must be bare HTTPS origins")]
    InsecureApiUrl,
    #[error("configuration path is unavailable; set AEX_CONFIG")]
    MissingConfigPath,
    #[error("configuration could not be read")]
    Read,
    #[error("configuration could not be written")]
    Write,
    #[error("configuration is malformed")]
    Malformed,
    #[error("unknown configuration key `{0}`")]
    UnknownKey(String),
    #[error(
        "secret values cannot be persisted; store only `api-key-ref=env:NAME` or a protected file reference"
    )]
    SecretPersistence,
    #[error("API key reference must be `env:NAME` or an absolute `file:/path`")]
    InvalidApiKeyReference,
    #[error("AEX_API_KEY is unset and the selected profile has no api-key-ref")]
    MissingApiKey,
    #[error("AEX_DASHBOARD_SESSION is unset and the selected profile has no dashboard-session-ref")]
    MissingDashboardSession,
    #[error("the API key reference could not be resolved")]
    ApiKeyUnavailable,
    #[error("a referenced credential file must be private to the current user")]
    InsecureCredentialFile,
}

/// Apply flag → environment → profile → built-in precedence per setting.
pub fn resolve_config(input: ConfigInputs) -> Result<ResolvedConfig, CliConfigError> {
    let central_url = input
        .central_url
        .or_else(|| input.env.get("AEX_CENTRAL_URL").cloned())
        .or(input.profile.central_url)
        .unwrap_or_else(|| "https://api.aex.dev".to_owned());
    let regional_url = input
        .regional_url
        .or_else(|| input.env.get("AEX_REGIONAL_URL").cloned())
        .or(input.profile.regional_url)
        .unwrap_or_else(|| "https://eu-west-1.api.aex.dev".to_owned());
    if !is_bare_https_origin(&central_url) || !is_bare_https_origin(&regional_url) {
        return Err(CliConfigError::InsecureApiUrl);
    }
    let output = if let Some(output) = input.output {
        output
    } else if let Some(value) = input.env.get("AEX_OUTPUT") {
        value
            .parse()
            .map_err(|()| CliConfigError::InvalidOutput(value.clone()))?
    } else {
        input.profile.output.unwrap_or(if input.stdout_is_terminal {
            OutputFormat::Text
        } else {
            OutputFormat::Json
        })
    };
    Ok(ResolvedConfig {
        central_url,
        regional_url,
        output,
    })
}

fn is_bare_https_origin(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    !authority.is_empty() && !authority.contains(['/', '?', '#'])
}

/// Selects the config path without creating it.
pub fn config_path(explicit: Option<&Path>) -> Result<PathBuf, CliConfigError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("AEX_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(root).join("aex").join("config.json"));
    }
    if let Some(root) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(root).join("aex").join("config.json"));
    }
    if let Some(root) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(root)
            .join(".config")
            .join("aex")
            .join("config.json"));
    }
    Err(CliConfigError::MissingConfigPath)
}

/// Loads the selected profile. A missing file/profile is an empty profile.
pub fn load_profile(path: &Path, name: &str) -> Result<Profile, CliConfigError> {
    Ok(load_file(path)?.profiles.remove(name).unwrap_or_default())
}

pub fn load_file(path: &Path) -> Result<ConfigFile, CliConfigError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| CliConfigError::Malformed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(_) => Err(CliConfigError::Read),
    }
}

/// Writes one admitted non-secret profile key atomically.
pub fn set_value(
    path: &Path,
    profile_name: &str,
    key: &str,
    value: &str,
) -> Result<(), CliConfigError> {
    reject_secret_key(key)?;
    let mut file = load_file(path)?;
    let profile = file.profiles.entry(profile_name.to_owned()).or_default();
    match key {
        "central-url" => profile.central_url = Some(validate_origin(value)?),
        "regional-url" => profile.regional_url = Some(validate_origin(value)?),
        "output" => {
            profile.output = Some(
                value
                    .parse()
                    .map_err(|()| CliConfigError::InvalidOutput(value.to_owned()))?,
            );
        }
        "api-key-ref" => {
            validate_credential_ref(value)?;
            profile.api_key_ref = Some(value.to_owned());
        }
        "dashboard-session-ref" => {
            validate_credential_ref(value)?;
            profile.dashboard_session_ref = Some(value.to_owned());
        }
        other => return Err(CliConfigError::UnknownKey(other.to_owned())),
    }
    write_file(path, &file)
}

/// Removes one admitted non-secret profile key.
pub fn unset_value(path: &Path, profile_name: &str, key: &str) -> Result<(), CliConfigError> {
    reject_secret_key(key)?;
    let mut file = load_file(path)?;
    let profile = file.profiles.entry(profile_name.to_owned()).or_default();
    match key {
        "central-url" => profile.central_url = None,
        "regional-url" => profile.regional_url = None,
        "output" => profile.output = None,
        "api-key-ref" => profile.api_key_ref = None,
        "dashboard-session-ref" => profile.dashboard_session_ref = None,
        other => return Err(CliConfigError::UnknownKey(other.to_owned())),
    }
    write_file(path, &file)
}

/// Returns a JSON-safe value for one key. References, never credential bytes,
/// are returned for `api-key-ref`.
pub fn get_value(profile: &Profile, key: &str) -> Result<Option<String>, CliConfigError> {
    reject_secret_key(key)?;
    match key {
        "central-url" => Ok(profile.central_url.clone()),
        "regional-url" => Ok(profile.regional_url.clone()),
        "output" => Ok(profile.output.map(|value| match value {
            OutputFormat::Text => "text".to_owned(),
            OutputFormat::Json => "json".to_owned(),
            OutputFormat::Ndjson => "ndjson".to_owned(),
        })),
        "api-key-ref" => Ok(profile.api_key_ref.clone()),
        "dashboard-session-ref" => Ok(profile.dashboard_session_ref.clone()),
        other => Err(CliConfigError::UnknownKey(other.to_owned())),
    }
}

/// Resolve API key material without retaining it in CLI state or Debug output.
pub fn resolve_api_key(
    profile: &Profile,
    env: &BTreeMap<String, String>,
) -> Result<Zeroizing<String>, CliConfigError> {
    let value = if let Some(value) = env.get("AEX_API_KEY") {
        non_empty_secret(value)?
    } else {
        let reference = profile
            .api_key_ref
            .as_deref()
            .ok_or(CliConfigError::MissingApiKey)?;
        resolve_reference(reference, env)?
    };
    if !value.starts_with("aex_wk_") {
        return Err(CliConfigError::ApiKeyUnavailable);
    }
    Ok(value)
}

/// Resolve a dashboard-session credential for central billing routes. This is
/// independent from the workspace API key used by regional routes.
pub fn resolve_dashboard_session(
    profile: &Profile,
    env: &BTreeMap<String, String>,
) -> Result<Zeroizing<String>, CliConfigError> {
    let value = if let Some(value) = env.get("AEX_DASHBOARD_SESSION") {
        non_empty_secret(value)?
    } else {
        let reference = profile
            .dashboard_session_ref
            .as_deref()
            .ok_or(CliConfigError::MissingDashboardSession)?;
        resolve_reference(reference, env)?
    };
    if !value.starts_with("aex_ds_") {
        return Err(CliConfigError::ApiKeyUnavailable);
    }
    Ok(value)
}

fn resolve_reference(
    reference: &str,
    env: &BTreeMap<String, String>,
) -> Result<Zeroizing<String>, CliConfigError> {
    if let Some(name) = reference.strip_prefix("env:") {
        let value = env.get(name).ok_or(CliConfigError::ApiKeyUnavailable)?;
        return non_empty_secret(value);
    }
    if let Some(raw) = reference.strip_prefix("file:") {
        let path = Path::new(raw);
        validate_credential_file(path)?;
        let value = fs::read_to_string(path).map_err(|_| CliConfigError::ApiKeyUnavailable)?;
        return non_empty_secret(value.trim());
    }
    Err(CliConfigError::InvalidApiKeyReference)
}

fn non_empty_secret(value: &str) -> Result<Zeroizing<String>, CliConfigError> {
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(CliConfigError::ApiKeyUnavailable);
    }
    Ok(Zeroizing::new(value.to_owned()))
}

fn validate_origin(value: &str) -> Result<String, CliConfigError> {
    if is_bare_https_origin(value) {
        Ok(value.strip_suffix('/').unwrap_or(value).to_owned())
    } else {
        Err(CliConfigError::InsecureApiUrl)
    }
}

fn reject_secret_key(key: &str) -> Result<(), CliConfigError> {
    let lower = key.to_ascii_lowercase();
    if ["provider", "mcp", "secret", "token", "credential"]
        .iter()
        .any(|part| lower.contains(part))
        || (lower.contains("api-key") && lower != "api-key-ref")
        || (lower.contains("dashboard-session") && lower != "dashboard-session-ref")
    {
        return Err(CliConfigError::SecretPersistence);
    }
    Ok(())
}

fn validate_credential_ref(value: &str) -> Result<(), CliConfigError> {
    if let Some(name) = value.strip_prefix("env:") {
        if !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Ok(());
        }
    }
    if let Some(path) = value.strip_prefix("file:") {
        if Path::new(path).is_absolute() {
            return Ok(());
        }
    }
    Err(CliConfigError::InvalidApiKeyReference)
}

fn write_file(path: &Path, file: &ConfigFile) -> Result<(), CliConfigError> {
    let parent = path.parent().ok_or(CliConfigError::Write)?;
    fs::create_dir_all(parent).map_err(|_| CliConfigError::Write)?;
    let temporary = path.with_extension("json.part");
    let bytes = serde_json::to_vec_pretty(file).map_err(|_| CliConfigError::Malformed)?;
    fs::write(&temporary, bytes).map_err(|_| CliConfigError::Write)?;
    set_private_permissions(&temporary)?;
    fs::rename(&temporary, path).map_err(|_| CliConfigError::Write)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), CliConfigError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| CliConfigError::Write)
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), CliConfigError> {
    Ok(())
}

#[cfg(unix)]
fn validate_credential_file(path: &Path) -> Result<(), CliConfigError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(path)
        .map_err(|_| CliConfigError::ApiKeyUnavailable)?
        .permissions()
        .mode();
    if mode & 0o077 == 0 {
        Ok(())
    } else {
        Err(CliConfigError::InsecureCredentialFile)
    }
}

#[cfg(not(unix))]
fn validate_credential_file(_path: &Path) -> Result<(), CliConfigError> {
    // The Rust standard library cannot prove a Windows DACL is owner-only.
    // Environment references remain available; an unverifiable file fails
    // closed rather than being called protected.
    Err(CliConfigError::InsecureCredentialFile)
}

/// Unix credential files must be readable only by their owner.
#[must_use]
pub const fn credentials_mode_is_private(mode: u32) -> bool {
    mode & 0o077 == 0
}
