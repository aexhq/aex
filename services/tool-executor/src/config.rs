//! Start-up configuration. Everything is required and nothing has a default.
//!
//! A default here would be a plane running a band nobody approved, and the
//! failure mode of the alternative — a task that refuses to start — is the one
//! an operator can see.

use std::collections::BTreeMap;

use aex_identity_domain::assertion::{
    KeyId, MAX_VERIFICATION_KEYS, Plane, VerificationKey, VerificationKeySet,
};
use aex_wire::types::Region;

use crate::spend::Ceilings;

/// The environment namespace every key shares.
pub const ENV_PREFIX: &str = "AEX_TOOL_EXECUTOR_";

/// The most calls this process will run at once.
///
/// **64, and the number is derived from the fleet rather than from this
/// process.** `brain-mux` runs a fixed two tasks in production — its Terraform
/// forces both autoscaling bounds equal to `desired_count` — and each holds a
/// 128-unit network lane against a declared tool weight of 4, so each can have
/// at most 32 platform tool calls outstanding. Two times thirty-two is
/// sixty-four, which is every call the deployed Brain fleet can produce at once.
///
/// Sizing the executor's admission *at* that number rather than below it is the
/// whole point: a tool call holds an activation, a lease and a lane permit while
/// it waits, so an executor that queued would extend every one of those holds
/// and would become the thing that makes the fleet's fixed 2 x 16 activation
/// ceiling bind. At 64 it cannot queue, because there is nothing left to queue.
pub const MAX_IN_FLIGHT: usize = 64;

/// Why start-up was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A required setting was absent or empty.
    #[error("`{key}` is required and was not set")]
    Missing {
        /// The full environment key.
        key: String,
    },
    /// A setting was present and unusable.
    #[error("`{key}` is not valid: {reason}")]
    Invalid {
        /// The full environment key.
        key: String,
        /// What was wrong with it.
        reason: String,
    },
    /// The verification key set was refused.
    #[error("the verification key set was refused: {0}")]
    KeySet(String),
}

/// The settings this deployable runs on.
#[derive(Debug, Clone)]
pub struct Config {
    /// Which plane.
    pub plane: Plane,
    /// Which region.
    pub region: Region,
    /// The port the private listener binds.
    pub port: u16,
    /// The `DynamoDB` table holding the per-organization window counters.
    pub ceiling_table: String,
    /// The Secrets Manager id of the platform's own search credential.
    pub credential_secret_id: String,
    /// The keys whose signatures this process accepts.
    pub verification_keys: VerificationKeySet,
    /// The ceilings this deployment enforces.
    pub ceilings: Ceilings,
}

impl Config {
    /// Reads the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for the first missing or unusable setting. There
    /// is no partially-valid configuration: a process that started on one is a
    /// process whose behaviour nobody chose.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }

    /// The same resolution over an explicit map, so a test can drive it without
    /// mutating the process environment.
    ///
    /// # Errors
    ///
    /// As [`Config::from_env`].
    pub fn from_map(vars: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let read = |suffix: &str| -> Result<String, ConfigError> {
            let key = format!("{ENV_PREFIX}{suffix}");
            vars.get(&key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .ok_or(ConfigError::Missing { key })
        };
        let invalid = |suffix: &str, reason: &str| ConfigError::Invalid {
            key: format!("{ENV_PREFIX}{suffix}"),
            reason: reason.to_owned(),
        };

        let plane = Plane::parse(&read("PLANE")?)
            .ok_or_else(|| invalid("PLANE", "expected `dev` or `prd`"))?;
        let region_text = read("REGION")?;
        let region = Region::from_name(&region_text)
            .ok_or_else(|| invalid("REGION", "expected a known AWS region name"))?;
        let port = read("PORT")?
            .parse::<u16>()
            .map_err(|_| invalid("PORT", "expected a TCP port"))?;

        Ok(Self {
            plane,
            region,
            port,
            ceiling_table: read("CEILING_TABLE")?,
            credential_secret_id: read("CREDENTIAL_SECRET_ID")?,
            verification_keys: parse_key_set(&read("VERIFICATION_KEYS")?)?,
            ceilings: Ceilings::DEFAULT,
        })
    }
}

/// Parses the verification key set.
///
/// One `kid:public_key:not_after_ms` triple per key, semicolon separated, both
/// binary halves in lowercase hexadecimal. A list rather than a single key
/// because a rotation has two keys live at once, and the alternative — restart
/// the fleet between halves of a rotation — is how a rotation becomes an outage.
fn parse_key_set(raw: &str) -> Result<VerificationKeySet, ConfigError> {
    let invalid = |reason: &str| ConfigError::Invalid {
        key: format!("{ENV_PREFIX}VERIFICATION_KEYS"),
        reason: reason.to_owned(),
    };

    let mut keys = Vec::new();
    for entry in raw.split(';').map(str::trim).filter(|it| !it.is_empty()) {
        let mut parts = entry.split(':');
        let (Some(kid), Some(public), Some(not_after), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid("expected `kid:public_key:not_after_ms` triples"));
        };
        let kid = uuid::Uuid::parse_str(kid).map_err(|_| invalid("the key id is not a UUID"))?;
        let decoded = decode_hex(public).ok_or_else(|| invalid("the public key is not hex"))?;
        let public_key = <[u8; 32]>::try_from(decoded.as_slice())
            .map_err(|_| invalid("an Ed25519 public key is 32 bytes"))?;
        let not_after_ms = not_after
            .parse::<u64>()
            .map_err(|_| invalid("the expiry is not a millisecond timestamp"))?;
        keys.push(VerificationKey {
            kid: KeyId::new(kid),
            public_key,
            not_after_ms,
        });
    }
    if keys.is_empty() {
        return Err(invalid("at least one verification key is required"));
    }
    if keys.len() > MAX_VERIFICATION_KEYS {
        return Err(invalid("more keys than the envelope's bound admits"));
    }
    VerificationKeySet::new(keys).map_err(|error| ConfigError::KeySet(error.to_string()))
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}
