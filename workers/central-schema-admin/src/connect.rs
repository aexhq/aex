//! Credential resolution, TLS and the outer advisory lock.
//!
//! This is the only identity in the platform with DDL privilege, so everything
//! here is deliberately narrow: one secret, one pinned CA bundle, one
//! `verify-full` connection, one fixed lock key. The credential is held in a
//! `Zeroizing<String>` and never enters argv, output or a log line.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use sqlx::ConnectOptions as _;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgSslMode};
use zeroize::Zeroizing;

/// The fixed outer advisory lock, ASCII `AEX_MIGR`.
///
/// A constant rather than a name-derived hash: a hash can collide across
/// databases, and an auditor cannot check a number nobody wrote down.
pub const ADVISORY_LOCK_KEY: i64 = 0x4145_585F_4D49_4752;

/// What the admin secret carries.
///
/// Only the two fields this task needs are modelled; anything else the secret
/// holds is ignored rather than logged.
#[derive(Debug, Deserialize)]
pub struct AdminCredential {
    /// The DDL role to connect as.
    pub username: String,
    /// That role's password.
    pub password: String,
}

impl AdminCredential {
    /// Parses the secret payload.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectError::SecretShape`] when the payload is not the
    /// expected object. The payload itself never appears in the message.
    pub fn parse(payload: &Zeroizing<String>) -> Result<Zeroizing<Self>, ConnectError> {
        serde_json::from_str::<Self>(payload)
            .map(Zeroizing::new)
            .map_err(|_| ConnectError::SecretShape)
    }
}

impl zeroize::Zeroize for AdminCredential {
    fn zeroize(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

/// Everything needed to reach one cluster as the DDL role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The `PostgreSQL` host.
    pub host: String,
    /// The `PostgreSQL` port.
    pub port: u16,
    /// The database name.
    pub database: String,
    /// The pinned RDS root CA bundle.
    pub tls_root_ca_path: std::path::PathBuf,
    /// How long a connection attempt may take.
    pub connect_timeout: Duration,
}

/// Why the task could not reach the database.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The secret could not be read.
    #[error("the DDL credential is unavailable: {0}")]
    SecretUnavailable(String),
    /// The secret is not the expected object.
    #[error("the DDL credential is not a `{{username, password}}` object")]
    SecretShape,
    /// The pinned CA bundle is absent or unreadable.
    #[error("the pinned RDS root CA bundle is unreadable at `{0}`")]
    RootCaUnreadable(String),
    /// The connection or its TLS handshake failed.
    #[error("the database connection failed: {0}")]
    Connection(String),
    /// Another task already holds the outer lock.
    #[error("another schema task holds the outer advisory lock")]
    LockUnavailable,
}

/// Resolves the DDL credential from Secrets Manager.
///
/// # Errors
///
/// Returns [`ConnectError::SecretUnavailable`] when the secret cannot be read
/// and [`ConnectError::SecretShape`] when its payload is not the expected
/// object. Neither message contains the payload.
pub async fn resolve_credential(
    client: &aws_sdk_secretsmanager::Client,
    secret_arn: &str,
) -> Result<Zeroizing<AdminCredential>, ConnectError> {
    let answer = client
        .get_secret_value()
        .secret_id(secret_arn)
        .send()
        .await
        .map_err(|error| {
            ConnectError::SecretUnavailable(
                aws_sdk_secretsmanager::error::DisplayErrorContext(&error).to_string(),
            )
        })?;
    let payload = Zeroizing::new(
        answer
            .secret_string()
            .ok_or_else(|| {
                ConnectError::SecretUnavailable("the secret carries no string payload".to_owned())
            })?
            .to_owned(),
    );
    AdminCredential::parse(&payload)
}

/// Builds the `verify-full` connection options for one endpoint.
///
/// # Errors
///
/// Returns [`ConnectError::RootCaUnreadable`] when the pinned bundle is absent.
/// There is deliberately no arm that falls back to a weaker verification mode:
/// a DDL session that cannot verify its server is a session that must not open.
pub fn options(
    endpoint: &Endpoint,
    credential: &AdminCredential,
) -> Result<PgConnectOptions, ConnectError> {
    if !Path::new(&endpoint.tls_root_ca_path).is_file() {
        return Err(ConnectError::RootCaUnreadable(
            endpoint.tls_root_ca_path.display().to_string(),
        ));
    }
    Ok(PgConnectOptions::new()
        .host(&endpoint.host)
        .port(endpoint.port)
        .database(&endpoint.database)
        .username(&credential.username)
        .password(&credential.password)
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(&endpoint.tls_root_ca_path)
        .application_name("central-schema-admin")
        .disable_statement_logging())
}

/// Opens one DDL session.
///
/// # Errors
///
/// Returns [`ConnectError::Connection`] for any connection or handshake failure.
pub async fn connect(
    endpoint: &Endpoint,
    credential: &AdminCredential,
) -> Result<PgConnection, ConnectError> {
    let options = options(endpoint, credential)?;
    tokio::time::timeout(endpoint.connect_timeout, options.connect())
        .await
        .map_err(|_| ConnectError::Connection("the connection attempt timed out".to_owned()))?
        .map_err(|error| ConnectError::Connection(error.to_string()))
}

/// Takes the outer session lock without waiting.
///
/// A concurrent second task exits rather than queueing, which closes the
/// inherited gap where concurrency relied only on a reserved concurrency of one.
///
/// # Errors
///
/// Returns [`ConnectError::LockUnavailable`] when another session holds it and
/// [`ConnectError::Connection`] when the probe itself fails.
pub async fn take_outer_lock(connection: &mut PgConnection) -> Result<(), ConnectError> {
    let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| ConnectError::Connection(error.to_string()))?;
    if taken {
        Ok(())
    } else {
        Err(ConnectError::LockUnavailable)
    }
}

/// Applies the statement and lock timeouts for one run.
///
/// # Errors
///
/// Returns [`ConnectError::Connection`] when the session refuses the setting.
pub async fn apply_timeouts(
    connection: &mut PgConnection,
    lock_timeout_ms: u64,
    statement_timeout_ms: u64,
) -> Result<(), ConnectError> {
    // `SET` admits no bound parameter, and both values are `u64` already parsed
    // by `clap`, so the only thing interpolated is a decimal integer. That is
    // the audit the assertion below records.
    for statement in [
        format!("SET lock_timeout = {lock_timeout_ms}"),
        format!("SET statement_timeout = {statement_timeout_ms}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut *connection)
            .await
            .map_err(|error| ConnectError::Connection(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::{ADVISORY_LOCK_KEY, AdminCredential, ConnectError, Endpoint, options};

    fn endpoint(path: &str) -> Endpoint {
        Endpoint {
            host: "db.example".to_owned(),
            port: 5432,
            database: "aex".to_owned(),
            tls_root_ca_path: path.into(),
            connect_timeout: std::time::Duration::from_secs(10),
        }
    }

    #[test]
    fn the_advisory_lock_key_is_the_fixed_ascii_constant() {
        assert_eq!(ADVISORY_LOCK_KEY, 0x4145_585F_4D49_4752);
        assert_eq!(
            ADVISORY_LOCK_KEY.to_be_bytes(),
            *b"AEX_MIGR",
            "the key is auditable because it spells its own purpose"
        );
    }

    #[test]
    fn a_secret_that_is_not_the_expected_object_is_refused_without_echoing_it() {
        let payload = Zeroizing::new(r#"{"secret":"hunter2"}"#.to_owned());
        let error = AdminCredential::parse(&payload).expect_err("the shape is wrong");
        assert!(matches!(error, ConnectError::SecretShape));
        assert!(
            !error.to_string().contains("hunter2"),
            "a credential never appears in a message"
        );
    }

    #[test]
    fn a_well_formed_secret_parses() {
        let payload =
            Zeroizing::new(r#"{"username":"aex_schema_admin","password":"x"}"#.to_owned());
        let credential = AdminCredential::parse(&payload).expect("the shape is right");
        assert_eq!(credential.username, "aex_schema_admin");
    }

    #[test]
    fn an_absent_root_ca_bundle_refuses_the_session_rather_than_weakening_it() {
        let payload =
            Zeroizing::new(r#"{"username":"aex_schema_admin","password":"x"}"#.to_owned());
        let credential = AdminCredential::parse(&payload).expect("a credential");
        let error = options(&endpoint("no-such-bundle.pem"), &credential)
            .expect_err("the pinned bundle is absent");
        assert!(matches!(error, ConnectError::RootCaUnreadable(_)));
    }
}
