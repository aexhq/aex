//! Starting one pinned engine, with a real readiness signal.
//!
//! [`crate::images`] answers *which* image an engine runs. This module answers
//! *how to run it*: a stream that needs a real engine calls one `start`
//! function and receives a handle carrying the mapped host port and a
//! ready-to-use endpoint. It never names an image, never chooses a port, never
//! writes a wait loop and never reaches a host-local service. A start either
//! yields a container running the digest-pinned image or a [`ContainerError`]
//! naming the engine and the cause; there is no third outcome and no fallback.
//!
//! # Readiness, not sleeping
//!
//! A container that has been *created* is not a container that *accepts
//! connections*, and the gap between the two is the classic integration flake.
//! Every engine therefore declares a [`Readiness`] condition observed on the
//! engine itself - a log marker it writes, an HTTP answer only its own
//! request path can produce, or a Docker health check that runs the engine's
//! own probe. None of them is a sleep, and none of them is a retry loop around
//! the assertion.
//!
//! # Feature
//!
//! Everything that talks to Docker sits behind the non-default `containers`
//! feature, so the ordinary unit lane links neither `testcontainers` nor a
//! Docker client. The engine table, the endpoint shaping and the error type are
//! always compiled, which is what lets the hermetic lane cover them.
//!
//! # Not this module's job
//!
//! - creating tables, buckets, queues or schemas: the product's own migration
//!   or provisioning code does that, against the endpoint on the handle;
//! - cleaning up: the container dies with the handle, and anything that
//!   outlives the process belongs in [`crate::ledger`];
//! - deciding that a local engine proved an AWS behaviour: every entry in
//!   `release/policy/test-images.toml` records what it `cannot_prove`.

use std::time::Duration;

use crate::images::{self, ImageError};

#[cfg(feature = "containers")]
use testcontainers::core::wait::HttpWaitStrategy;
#[cfg(feature = "containers")]
use testcontainers::core::{Healthcheck, IntoContainerPort, WaitFor};
#[cfg(feature = "containers")]
use testcontainers::runners::AsyncRunner;
#[cfg(feature = "containers")]
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// The role a started `PostgreSQL` owns its database as.
pub const POSTGRES_USER: &str = "aex";
/// That role's password.
pub const POSTGRES_PASSWORD: &str = "aex-harness-password";
/// The database created on first start.
pub const POSTGRES_DATABASE: &str = "aex";

/// The root user a started `MinIO` accepts.
pub const MINIO_ACCESS_KEY_ID: &str = "aexharnessroot";
/// That user's secret. `MinIO` refuses a secret shorter than eight bytes.
pub const MINIO_SECRET_ACCESS_KEY: &str = "aexharnesssecret";

/// The access key id the emulated AWS engines accept.
///
/// `DynamoDB` Local with `-sharedDb` and `LocalStack` both ignore the value and
/// only require that one is present, so it is fixed here rather than invented
/// per call site: two clients signing with different keys must still reach the
/// same data.
pub const EMULATED_ACCESS_KEY_ID: &str = "aexharness";
/// The secret the emulated AWS engines accept.
pub const EMULATED_SECRET_ACCESS_KEY: &str = "aexharnesssecret";
/// The region every emulated client signs for; the planes' own region.
pub const EMULATED_REGION: &str = "eu-west-1";

/// The AWS services a started `LocalStack` loads eagerly.
///
/// Exactly the three the policy document says `LocalStack` may stand in for.
pub const LOCALSTACK_SERVICES: &str = "sqs,kms,secretsmanager";

/// How long an engine has to become ready before the start is a failure.
///
/// Generous enough for a cold `LocalStack` on a loaded host and still bounded:
/// a hang is reported as a hang rather than waited out forever.
pub const STARTUP_TIMEOUT: Duration = Duration::from_mins(3);

/// The probe command `PostgreSQL` is considered ready by.
///
/// Deliberately over the loopback TCP listener rather than the Unix socket: the
/// image's own initialisation runs a temporary server with `listen_addresses`
/// empty, so a socket probe passes while the port the test connects to is still
/// closed.
const POSTGRES_HEALTH_COMMAND: &str =
    "pg_isready --host=127.0.0.1 --port=5432 --username=aex --dbname=aex";

/// How often Docker runs a health check.
#[cfg(feature = "containers")]
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How long one health check may take.
#[cfg(feature = "containers")]
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// How many consecutive failures make a container unhealthy.
///
/// The product of the interval and this count exceeds [`STARTUP_TIMEOUT`], so
/// the timeout is what bounds a slow start, not an unhealthy verdict from a
/// probe that simply had not succeeded yet.
#[cfg(feature = "containers")]
const HEALTH_CHECK_RETRIES: u32 = 960;

/// The environment `PostgreSQL` is started with.
const POSTGRES_ENVIRONMENT: [(&str, &str); 3] = [
    ("POSTGRES_USER", POSTGRES_USER),
    ("POSTGRES_PASSWORD", POSTGRES_PASSWORD),
    ("POSTGRES_DB", POSTGRES_DATABASE),
];

/// The environment `MinIO` is started with.
const MINIO_ENVIRONMENT: [(&str, &str); 2] = [
    ("MINIO_ROOT_USER", MINIO_ACCESS_KEY_ID),
    ("MINIO_ROOT_PASSWORD", MINIO_SECRET_ACCESS_KEY),
];

/// The environment `LocalStack` is started with.
const LOCALSTACK_ENVIRONMENT: [(&str, &str); 4] = [
    ("SERVICES", LOCALSTACK_SERVICES),
    ("EAGER_SERVICE_LOADING", "1"),
    ("AWS_DEFAULT_REGION", EMULATED_REGION),
    ("DEBUG", "0"),
];

/// The argument vector `DynamoDB` Local is started with.
///
/// `-inMemory` keeps a run hermetic and `-sharedDb` puts every credential and
/// region in one database, so a client that signs with a different key still
/// sees the table the test created.
const DYNAMODB_LOCAL_COMMAND: [&str; 6] = [
    "-jar",
    "DynamoDBLocal.jar",
    "-inMemory",
    "-sharedDb",
    "-port",
    "8000",
];

/// The argument vector `MinIO` is started with.
const MINIO_COMMAND: [&str; 4] = ["server", "/data", "--address", ":9000"];

/// One engine of the pinned local substrate.
///
/// The variants are exactly the engines a stream can start today. Adding one
/// means adding a pinned entry to `release/policy/test-images.toml` first: an
/// engine with no digest has no way to reach this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Engine {
    /// `PostgreSQL`, standing in for Aurora under the central and finance
    /// schemas.
    Postgres,
    /// `DynamoDB` Local, standing in for the regional authority tables.
    DynamoDbLocal,
    /// `MinIO`, standing in for S3 under content, checkpoint and export
    /// objects.
    Minio,
    /// `LocalStack`, standing in for SQS, KMS and Secrets Manager.
    LocalStack,
}

impl Engine {
    /// Every engine, in policy-key order.
    pub const ALL: [Self; 4] = [
        Self::DynamoDbLocal,
        Self::LocalStack,
        Self::Minio,
        Self::Postgres,
    ];

    /// The engine's key in `release/policy/test-images.toml`.
    #[must_use]
    pub const fn image_key(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::DynamoDbLocal => "dynamodb_local",
            Self::Minio => "minio",
            Self::LocalStack => "localstack",
        }
    }

    /// The port the engine listens on inside its container.
    #[must_use]
    pub const fn container_port(self) -> u16 {
        match self {
            Self::Postgres => 5432,
            Self::DynamoDbLocal => 8000,
            Self::Minio => 9000,
            Self::LocalStack => 4566,
        }
    }

    /// The environment the container is started with.
    #[must_use]
    pub const fn environment(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Postgres => &POSTGRES_ENVIRONMENT,
            Self::DynamoDbLocal => &[],
            Self::Minio => &MINIO_ENVIRONMENT,
            Self::LocalStack => &LOCALSTACK_ENVIRONMENT,
        }
    }

    /// The argument vector that overrides the image's own, if any.
    #[must_use]
    pub const fn command(self) -> &'static [&'static str] {
        match self {
            Self::Postgres | Self::LocalStack => &[],
            Self::DynamoDbLocal => &DYNAMODB_LOCAL_COMMAND,
            Self::Minio => &MINIO_COMMAND,
        }
    }

    /// What must be observed, in order, before the engine is ready.
    #[must_use]
    pub const fn readiness(self) -> &'static [Readiness] {
        match self {
            Self::Postgres => &[Readiness::HealthCheck {
                command: POSTGRES_HEALTH_COMMAND,
            }],
            // An unauthenticated request is answered `400
            // MissingAuthenticationToken` by the request dispatcher itself, so
            // this status proves the engine - not merely the port - is live.
            Self::DynamoDbLocal => &[Readiness::HttpStatus {
                path: "/",
                status: 400,
            }],
            Self::Minio => &[Readiness::HttpStatus {
                path: "/minio/health/live",
                status: 200,
            }],
            // The marker is written once every eagerly loaded service is
            // running; the health probe then confirms the gateway answers.
            Self::LocalStack => &[
                Readiness::LogMarker("Ready."),
                Readiness::HttpStatus {
                    path: "/_localstack/health",
                    status: 200,
                },
            ],
        }
    }

    /// The engine's digest-pinned image reference.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::Image`] when the policy document has no entry
    /// for the engine or the entry carries no digest.
    pub fn reference(self) -> Result<String, ContainerError> {
        images::reference(self.image_key()).map_err(|source| ContainerError::Image {
            engine: self,
            source,
        })
    }

    /// The image name and tag whose colon-join reproduces [`Self::reference`].
    ///
    /// `testcontainers` addresses an image as a `name` and a `tag` and joins
    /// them with a colon, so a digest reference is carried as the
    /// `repository@algorithm` name and the hex digest as the tag. Splitting
    /// here is what keeps the digest the only thing a caller can run.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::Image`] when the engine has no pinned entry,
    /// or [`ContainerError::NotDigestPinned`] when the entry resolved to
    /// something that is not `registry/repository@algorithm:digest`.
    pub fn image_name_and_tag(self) -> Result<(String, String), ContainerError> {
        let reference = self.reference()?;
        let (name, tag) = split_digest_reference(self, &reference)?;
        Ok((name.to_owned(), tag.to_owned()))
    }

    /// The Docker health check the engine's readiness needs, if any.
    #[cfg(feature = "containers")]
    fn health_check(self) -> Option<Healthcheck> {
        self.readiness()
            .iter()
            .find_map(|condition| match *condition {
                Readiness::HealthCheck { command } => Some(
                    Healthcheck::cmd_shell(command)
                        .with_interval(HEALTH_CHECK_INTERVAL)
                        .with_timeout(HEALTH_CHECK_TIMEOUT)
                        .with_retries(HEALTH_CHECK_RETRIES)
                        .with_start_period(Duration::ZERO),
                ),
                Readiness::LogMarker(_) | Readiness::HttpStatus { .. } => None,
            })
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.image_key())
    }
}

/// One condition that must hold before an engine is handed to a test.
///
/// Every variant observes the engine. There is deliberately no "wait n
/// seconds": a duration is not evidence, and a start that needs one is a start
/// whose readiness signal has not been found yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The engine has written this marker to stdout or stderr.
    LogMarker(&'static str),
    /// An HTTP `GET` of `path` on the mapped port answers `status`.
    HttpStatus {
        /// The request path, relative to the engine's origin.
        path: &'static str,
        /// The status only a live engine produces.
        status: u16,
    },
    /// The engine's own probe, run by Docker as a container health check,
    /// reports healthy.
    HealthCheck {
        /// The shell command Docker runs.
        command: &'static str,
    },
}

/// Why an engine could not be started.
///
/// Every variant names the engine, so a failure in a suite that starts four of
/// them says which one broke. None of them is recoverable by falling back to an
/// unpinned image or to a service already listening on the host: a test that
/// silently ran against the developer's own database proves nothing.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// The engine has no runnable image in the policy document.
    #[error("engine `{engine}` has no runnable image: {source}")]
    Image {
        /// The engine that cannot start.
        engine: Engine,
        /// Why the registry refused.
        #[source]
        source: ImageError,
    },
    /// The policy entry resolved to something that is not a digest reference.
    #[error(
        "engine `{engine}` resolved to `{reference}`, which is not a `registry/repository@algorithm:digest` reference; an engine is started by digest or not at all"
    )]
    NotDigestPinned {
        /// The engine that cannot start.
        engine: Engine,
        /// What the registry produced.
        reference: String,
    },
    /// Docker refused the container, or it never became ready in time.
    #[cfg(feature = "containers")]
    #[error(
        "engine `{engine}` did not become ready from `{reference}` within {}s: {source}",
        STARTUP_TIMEOUT.as_secs()
    )]
    Start {
        /// The engine that cannot start.
        engine: Engine,
        /// The digest-pinned reference that was asked for.
        reference: String,
        /// What Docker or the readiness wait reported.
        #[source]
        source: Box<testcontainers::TestcontainersError>,
    },
    /// The container runs but its port never mapped onto the host.
    #[cfg(feature = "containers")]
    #[error(
        "engine `{engine}` is running from `{reference}` but container port {container_port} is not reachable on the host: {source}"
    )]
    Endpoint {
        /// The engine that cannot be reached.
        engine: Engine,
        /// The digest-pinned reference it runs.
        reference: String,
        /// The port that should have been published.
        container_port: u16,
        /// What the Docker client reported.
        #[source]
        source: Box<testcontainers::TestcontainersError>,
    },
}

/// Splits `reference` into the name and tag `testcontainers` rejoins with a
/// colon.
fn split_digest_reference(engine: Engine, reference: &str) -> Result<(&str, &str), ContainerError> {
    let refuse = || ContainerError::NotDigestPinned {
        engine,
        reference: reference.to_owned(),
    };
    let at = reference.find('@').ok_or_else(refuse)?;
    let digest = reference.get(at + 1..).ok_or_else(refuse)?;
    let colon = digest.find(':').ok_or_else(refuse)?;
    let hex = digest.get(colon + 1..).ok_or_else(refuse)?;
    if at == 0 || colon == 0 || hex.is_empty() || hex.contains(':') {
        return Err(refuse());
    }
    let name = reference.get(..at + 1 + colon).ok_or_else(refuse)?;
    Ok((name, hex))
}

/// The `postgres://` URL for a `PostgreSQL` this module started at `host:port`.
#[must_use]
pub fn postgres_url(host: &str, port: u16) -> String {
    format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@{host}:{port}/{POSTGRES_DATABASE}")
}

/// The origin an AWS SDK client is pointed at for an engine at `host:port`.
///
/// No trailing slash and no path: an SDK appends its own, and a trailing slash
/// makes some clients sign a different canonical request than they send.
#[must_use]
pub fn http_endpoint(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

/// What every started engine has in common.
#[cfg(feature = "containers")]
#[derive(Debug)]
struct Started {
    engine: Engine,
    reference: String,
    host: String,
    port: u16,
    container: ContainerAsync<GenericImage>,
}

#[cfg(feature = "containers")]
impl Started {
    /// Pulls the pinned image, starts it and waits for its readiness condition.
    async fn launch(engine: Engine) -> Result<Self, ContainerError> {
        let reference = engine.reference()?;
        let (name, tag) = split_digest_reference(engine, &reference)?;
        let container_port = engine.container_port();

        let mut image = GenericImage::new(name.to_owned(), tag.to_owned())
            .with_exposed_port(container_port.tcp());
        for condition in engine.readiness() {
            image = image.with_wait_for(condition.wait_for(container_port));
        }

        let mut request = image.with_startup_timeout(STARTUP_TIMEOUT);
        for (key, value) in engine.environment() {
            request = request.with_env_var(*key, *value);
        }
        let command = engine.command();
        if !command.is_empty() {
            request = request.with_cmd(command.iter().copied());
        }
        if let Some(check) = engine.health_check() {
            request = request.with_health_check(check);
        }

        let container = request
            .start()
            .await
            .map_err(|source| ContainerError::Start {
                engine,
                reference: reference.clone(),
                source: Box::new(source),
            })?;

        let endpoint_failure =
            |source: testcontainers::TestcontainersError| ContainerError::Endpoint {
                engine,
                reference: reference.clone(),
                container_port,
                source: Box::new(source),
            };
        let host = container
            .get_host()
            .await
            .map_err(endpoint_failure)?
            .to_string();
        let port = container
            .get_host_port_ipv4(container_port.tcp())
            .await
            .map_err(endpoint_failure)?;

        Ok(Self {
            engine,
            reference,
            host,
            port,
            container,
        })
    }
}

#[cfg(feature = "containers")]
impl Readiness {
    /// This condition as a `testcontainers` wait strategy.
    fn wait_for(self, container_port: u16) -> WaitFor {
        match self {
            Self::LogMarker(marker) => WaitFor::message_on_either_std(marker),
            Self::HttpStatus { path, status } => WaitFor::http(
                HttpWaitStrategy::new(path)
                    .with_port(container_port.tcp())
                    .with_expected_status_code(status),
            ),
            Self::HealthCheck { .. } => WaitFor::healthcheck(),
        }
    }
}

/// Declares one engine handle and the accessors every handle shares.
#[cfg(feature = "containers")]
macro_rules! engine_handle {
    ($(#[$documentation:meta])* $name:ident, $engine:expr) => {
        $(#[$documentation])*
        ///
        /// The container is removed when the handle is dropped, so a test owns
        /// its engine for exactly as long as it holds this value.
        #[derive(Debug)]
        pub struct $name {
            inner: Started,
        }

        impl $name {
            /// Starts the pinned engine and returns once it is ready.
            ///
            /// # Errors
            ///
            /// Returns [`ContainerError`] when the image is not pinned, when
            /// Docker refuses the container, when the engine does not reach its
            /// [`Readiness`] condition within [`STARTUP_TIMEOUT`], or when the
            /// container's port is not published on the host. It never falls
            /// back to an unpinned image or to a service already listening on
            /// the host.
            pub async fn start() -> Result<Self, ContainerError> {
                Ok(Self {
                    inner: Started::launch($engine).await?,
                })
            }

            /// Which engine this handle runs.
            #[must_use]
            pub const fn engine(&self) -> Engine {
                self.inner.engine
            }

            /// The digest-pinned reference the container is running.
            #[must_use]
            pub fn reference(&self) -> &str {
                &self.inner.reference
            }

            /// The host the engine is reachable on.
            #[must_use]
            pub fn host(&self) -> &str {
                &self.inner.host
            }

            /// The host port the container port is published on.
            #[must_use]
            pub const fn port(&self) -> u16 {
                self.inner.port
            }

            /// The running container, for logs, `exec` and lifecycle control.
            #[must_use]
            pub const fn container(&self) -> &ContainerAsync<GenericImage> {
                &self.inner.container
            }
        }
    };
}

#[cfg(feature = "containers")]
engine_handle!(
    /// A running `PostgreSQL`, pinned by digest.
    PostgresContainer,
    Engine::Postgres
);

#[cfg(feature = "containers")]
engine_handle!(
    /// A running `DynamoDB` Local, pinned by digest.
    DynamoDbLocalContainer,
    Engine::DynamoDbLocal
);

#[cfg(feature = "containers")]
engine_handle!(
    /// A running `MinIO`, pinned by digest.
    MinioContainer,
    Engine::Minio
);

#[cfg(feature = "containers")]
engine_handle!(
    /// A running `LocalStack`, pinned by digest.
    LocalStackContainer,
    Engine::LocalStack
);

#[cfg(feature = "containers")]
impl PostgresContainer {
    /// The connection URL for the database created at start.
    #[must_use]
    pub fn connection_url(&self) -> String {
        postgres_url(self.host(), self.port())
    }

    /// The role the connection URL authenticates as.
    #[must_use]
    pub const fn user(&self) -> &'static str {
        POSTGRES_USER
    }

    /// That role's password.
    #[must_use]
    pub const fn password(&self) -> &'static str {
        POSTGRES_PASSWORD
    }

    /// The database created at start.
    #[must_use]
    pub const fn database(&self) -> &'static str {
        POSTGRES_DATABASE
    }
}

#[cfg(feature = "containers")]
impl DynamoDbLocalContainer {
    /// The endpoint to point a `DynamoDB` client at.
    #[must_use]
    pub fn endpoint_url(&self) -> String {
        http_endpoint(self.host(), self.port())
    }

    /// The region the client signs for.
    #[must_use]
    pub const fn region(&self) -> &'static str {
        EMULATED_REGION
    }

    /// The access key id the client signs with.
    #[must_use]
    pub const fn access_key_id(&self) -> &'static str {
        EMULATED_ACCESS_KEY_ID
    }

    /// The secret the client signs with.
    #[must_use]
    pub const fn secret_access_key(&self) -> &'static str {
        EMULATED_SECRET_ACCESS_KEY
    }
}

#[cfg(feature = "containers")]
impl MinioContainer {
    /// The endpoint to point an S3 client at.
    ///
    /// The client must be configured for path-style addressing: this engine
    /// serves no virtual-host bucket names.
    #[must_use]
    pub fn endpoint_url(&self) -> String {
        http_endpoint(self.host(), self.port())
    }

    /// The region the client signs for.
    #[must_use]
    pub const fn region(&self) -> &'static str {
        EMULATED_REGION
    }

    /// The root access key id the client signs with.
    #[must_use]
    pub const fn access_key_id(&self) -> &'static str {
        MINIO_ACCESS_KEY_ID
    }

    /// The root secret the client signs with.
    #[must_use]
    pub const fn secret_access_key(&self) -> &'static str {
        MINIO_SECRET_ACCESS_KEY
    }
}

#[cfg(feature = "containers")]
impl LocalStackContainer {
    /// The endpoint to point an SQS, KMS or Secrets Manager client at.
    #[must_use]
    pub fn endpoint_url(&self) -> String {
        http_endpoint(self.host(), self.port())
    }

    /// The region the client signs for.
    #[must_use]
    pub const fn region(&self) -> &'static str {
        EMULATED_REGION
    }

    /// The access key id the client signs with.
    #[must_use]
    pub const fn access_key_id(&self) -> &'static str {
        EMULATED_ACCESS_KEY_ID
    }

    /// The secret the client signs with.
    #[must_use]
    pub const fn secret_access_key(&self) -> &'static str {
        EMULATED_SECRET_ACCESS_KEY
    }

    /// The services this engine was started with, comma separated.
    #[must_use]
    pub const fn services(&self) -> &'static str {
        LOCALSTACK_SERVICES
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ContainerError, EMULATED_REGION, Engine, MINIO_ACCESS_KEY_ID, MINIO_SECRET_ACCESS_KEY,
        POSTGRES_DATABASE, POSTGRES_PASSWORD, POSTGRES_USER, Readiness, http_endpoint,
        postgres_url, split_digest_reference,
    };
    use crate::images::{self, ImageError};

    #[test]
    fn every_engine_names_a_digest_pinned_entry_of_the_policy_document() {
        for engine in Engine::ALL {
            assert!(
                images::keys().contains(&engine.image_key()),
                "`{engine}` is not in release/policy/test-images.toml"
            );
            let reference = engine.reference().expect("a pinned engine resolves");
            assert!(reference.contains("@sha256:"), "{reference}");
        }
    }

    #[test]
    fn the_name_and_tag_pair_rejoins_into_the_digest_reference() {
        for engine in Engine::ALL {
            let reference = engine.reference().expect("a pinned engine resolves");
            let (name, tag) = engine.image_name_and_tag().expect("a pinned engine splits");
            assert_eq!(
                format!("{name}:{tag}"),
                reference,
                "`{engine}` would run something other than its pin"
            );
            assert!(name.ends_with("@sha256"), "{name}");
            assert!(!tag.contains(':'), "{tag}");
        }
    }

    #[test]
    fn a_reference_that_is_not_digest_pinned_is_refused_by_engine_name() {
        for candidate in [
            "registry.example/engine:1.2.3",
            "registry.example/engine@sha256",
            "registry.example/engine@sha256:",
            "@sha256:abc",
        ] {
            let error = split_digest_reference(Engine::Postgres, candidate)
                .expect_err("an unpinned reference is refused");
            assert!(
                matches!(error, ContainerError::NotDigestPinned { .. }),
                "{candidate}"
            );
            assert!(
                error
                    .to_string()
                    .starts_with("engine `postgres` resolved to"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_registry_failure_names_the_engine_and_keeps_the_cause() {
        let error = ContainerError::Image {
            engine: Engine::Minio,
            source: ImageError::Unknown {
                key: "minio".to_owned(),
                permitted: "postgres".to_owned(),
            },
        };
        assert_eq!(
            error.to_string(),
            "engine `minio` has no runnable image: `minio` is not in release/policy/test-images.toml; permitted keys: postgres"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn every_engine_has_its_own_image_key_and_its_own_port() {
        let mut keys: Vec<&str> = Engine::ALL
            .iter()
            .map(|engine| engine.image_key())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Engine::ALL.len());

        let mut ports: Vec<u16> = Engine::ALL
            .iter()
            .map(|engine| engine.container_port())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), Engine::ALL.len());
    }

    #[test]
    fn every_engine_waits_on_something_the_engine_itself_emits() {
        for engine in Engine::ALL {
            let conditions = engine.readiness();
            assert!(!conditions.is_empty(), "`{engine}` starts without waiting");
            for condition in conditions {
                match *condition {
                    Readiness::LogMarker(marker) => assert!(!marker.is_empty(), "{engine}"),
                    Readiness::HttpStatus { path, status } => {
                        assert!(path.starts_with('/'), "{engine}: {path}");
                        assert!((100..600).contains(&status), "{engine}: {status}");
                    }
                    Readiness::HealthCheck { command } => {
                        assert!(!command.trim().is_empty(), "{engine}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_argument_vector_that_pins_a_port_pins_the_declared_one() {
        for engine in [Engine::DynamoDbLocal, Engine::Minio] {
            let port = engine.container_port().to_string();
            assert!(
                engine
                    .command()
                    .iter()
                    .any(|argument| argument.contains(&port)),
                "`{engine}` is told to listen on a port it does not publish"
            );
        }
    }

    #[test]
    fn the_postgres_health_check_probes_the_credentials_the_container_is_given() {
        let Some(Readiness::HealthCheck { command }) =
            Engine::Postgres.readiness().first().copied()
        else {
            panic!("postgres is ready when its own probe says so");
        };
        assert!(command.contains(POSTGRES_USER), "{command}");
        assert!(command.contains(POSTGRES_DATABASE), "{command}");
        assert!(
            command.contains(&Engine::Postgres.container_port().to_string()),
            "{command}"
        );
        assert!(
            command.contains("127.0.0.1"),
            "a socket probe passes while the published port is still closed: {command}"
        );
    }

    #[test]
    fn each_engine_is_given_the_environment_its_image_demands() {
        let postgres: Vec<&str> = Engine::Postgres
            .environment()
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(
            postgres,
            vec!["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB"]
        );

        let minio: Vec<&str> = Engine::Minio
            .environment()
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(minio, vec!["MINIO_ROOT_USER", "MINIO_ROOT_PASSWORD"]);

        let localstack = Engine::LocalStack.environment();
        assert!(
            localstack
                .iter()
                .any(|(key, value)| *key == "SERVICES" && value.contains("sqs")),
            "{localstack:?}"
        );
        assert!(
            localstack
                .iter()
                .any(|(key, value)| *key == "AWS_DEFAULT_REGION" && *value == EMULATED_REGION),
            "{localstack:?}"
        );

        assert!(Engine::DynamoDbLocal.environment().is_empty());
    }

    #[test]
    fn the_minio_root_credentials_satisfy_minios_own_minimum_lengths() {
        assert!(MINIO_ACCESS_KEY_ID.len() >= 3, "{MINIO_ACCESS_KEY_ID}");
        assert!(MINIO_SECRET_ACCESS_KEY.len() >= 8);
    }

    #[test]
    fn the_postgres_url_carries_the_role_the_password_and_the_database() {
        assert_eq!(
            postgres_url("127.0.0.1", 55_432),
            "postgres://aex:aex-harness-password@127.0.0.1:55432/aex"
        );
        assert!(postgres_url("db", 5_432).contains(POSTGRES_PASSWORD));
    }

    #[test]
    fn an_endpoint_is_a_bare_http_origin() {
        assert_eq!(http_endpoint("127.0.0.1", 4_566), "http://127.0.0.1:4566");
        assert!(!http_endpoint("localhost", 9_000).ends_with('/'));
    }

    #[test]
    fn an_engine_renders_as_its_policy_key() {
        assert_eq!(Engine::DynamoDbLocal.to_string(), "dynamodb_local");
        assert_eq!(Engine::LocalStack.to_string(), "localstack");
        assert_eq!(Engine::Minio.to_string(), "minio");
        assert_eq!(Engine::Postgres.to_string(), "postgres");
    }
}

#[cfg(all(test, feature = "containers"))]
mod started_tests {
    use super::{
        DynamoDbLocalContainer, Engine, LocalStackContainer, MinioContainer, PostgresContainer,
        http_endpoint, postgres_url,
    };
    use std::net::TcpStream;

    /// Fails unless the published port accepts a connection right now.
    ///
    /// This is the whole point of the readiness conditions: after `start`
    /// returns there is nothing left to wait for, so one attempt is the
    /// assertion. A loop here would hide exactly the defect it would be
    /// covering up.
    fn accepts_a_connection(host: &str, port: u16) {
        TcpStream::connect((host, port))
            .unwrap_or_else(|error| panic!("{host}:{port} refused a connection: {error}"));
    }

    #[tokio::test]
    async fn a_started_postgres_accepts_a_connection_as_soon_as_start_returns() {
        let engine = PostgresContainer::start()
            .await
            .expect("the pinned postgres starts");
        assert_eq!(engine.engine(), Engine::Postgres);
        assert!(
            engine.reference().contains("@sha256:"),
            "{}",
            engine.reference()
        );
        assert_eq!(
            engine.connection_url(),
            postgres_url(engine.host(), engine.port())
        );
        accepts_a_connection(engine.host(), engine.port());
    }

    #[tokio::test]
    async fn a_started_dynamodb_local_accepts_a_connection_as_soon_as_start_returns() {
        let engine = DynamoDbLocalContainer::start()
            .await
            .expect("the pinned dynamodb local starts");
        assert_eq!(engine.engine(), Engine::DynamoDbLocal);
        assert!(
            engine.reference().contains("@sha256:"),
            "{}",
            engine.reference()
        );
        assert_eq!(
            engine.endpoint_url(),
            http_endpoint(engine.host(), engine.port())
        );
        accepts_a_connection(engine.host(), engine.port());
    }

    #[tokio::test]
    async fn a_started_minio_accepts_a_connection_as_soon_as_start_returns() {
        let engine = MinioContainer::start()
            .await
            .expect("the pinned minio starts");
        assert_eq!(engine.engine(), Engine::Minio);
        assert!(
            engine.reference().contains("@sha256:"),
            "{}",
            engine.reference()
        );
        assert_eq!(
            engine.endpoint_url(),
            http_endpoint(engine.host(), engine.port())
        );
        accepts_a_connection(engine.host(), engine.port());
    }

    #[tokio::test]
    async fn a_started_localstack_accepts_a_connection_as_soon_as_start_returns() {
        let engine = LocalStackContainer::start()
            .await
            .expect("the pinned localstack starts");
        assert_eq!(engine.engine(), Engine::LocalStack);
        assert!(
            engine.reference().contains("@sha256:"),
            "{}",
            engine.reference()
        );
        assert_eq!(
            engine.endpoint_url(),
            http_endpoint(engine.host(), engine.port())
        );
        accepts_a_connection(engine.host(), engine.port());
    }
}
