//! `hands-agent` composition root (guest rootfs binary).
//!
//! Exclusive responsibility: the credential-free guest-root protocol agent
//! process. It is PID 1 of the `MicroVM`, serves the five verbs and the provider
//! lifecycle hooks on one port, and supervises one process group per open
//! operation.
//!
//! # What it is, and what it is not
//!
//! A **usability component, not a trust boundary**. H-BOUNDARY grants the customer
//! real root inside the VM, so nothing this process reports is authority:
//! guest-reported completion is customer-controlled observation that Brain bounds,
//! digests and stores as tool output. Killing or rewriting the agent fails the
//! customer's own operation and authorizes nothing.
//!
//! It links no AWS SDK, holds no credential, reads no `AWS_*` variable and can
//! produce no billable fact. `crates/aex-hands-agent/tests/no_cloud_authority.rs`
//! and `no_guest_billing.rs` fail if any of that changes, and
//! `tests/boundary.rs` here fails if this binary's own closure changes.

mod execute;
mod host;
mod image;
mod serve;

use std::sync::Arc;

use aex_hands_agent::boot::{GUEST_ROOT_VAR, JOURNAL_ROOT_VAR, LISTEN_ADDR_VAR, REQUIRED_VARS};
use aex_hands_agent::journal::Journal;
use aex_hands_protocol::operation::GuestRoot;

/// Why `hands-agent` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandsAgentConfigError {
    /// A required variable was absent or empty.
    #[error("required environment variable `{name}` is missing")]
    Missing {
        /// The variable that must be supplied.
        name: &'static str,
    },
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// The variable that was rejected.
        name: &'static str,
        /// Why the supplied value was rejected.
        reason: String,
    },
}

/// Why `hands-agent` stopped.
#[derive(Debug, thiserror::Error)]
pub enum HandsAgentRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] HandsAgentConfigError),
    /// The journal could not be opened or replayed.
    #[error("the operation journal is unusable: {0}")]
    Journal(#[from] aex_hands_agent::journal::JournalError),
    /// The listener or the server stopped.
    #[error("the guest listener stopped: {reason}")]
    Listener {
        /// Why.
        reason: String,
    },
}

/// Validated start-up configuration.
///
/// Three values, none defaulted. The image writes all three, and the same
/// constants that write them are the ones `runtimes/hands-image` builds the rootfs
/// from, so the binary and the image cannot disagree about where the journal is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The address the agent listens on.
    pub listen: std::net::SocketAddr,
    /// The operation journal root.
    pub journal_root: String,
    /// The guest filesystem root.
    pub guest_root: GuestRoot,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// See [`HandsAgentConfigError`].
    pub fn from_env() -> Result<Self, HandsAgentConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// See [`HandsAgentConfigError`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, HandsAgentConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let raw_listen = required(&lookup, LISTEN_ADDR_VAR)?;
        let listen = raw_listen
            .parse::<std::net::SocketAddr>()
            .map_err(|error| HandsAgentConfigError::Invalid {
                name: LISTEN_ADDR_VAR,
                reason: format!("`{raw_listen}` is not a socket address: {error}"),
            })?;
        let journal_root = required(&lookup, JOURNAL_ROOT_VAR)?;
        let guest_root = required(&lookup, GUEST_ROOT_VAR)?;
        if !guest_root.starts_with('/') {
            return Err(HandsAgentConfigError::Invalid {
                name: GUEST_ROOT_VAR,
                reason: format!("`{guest_root}` is not an absolute path"),
            });
        }
        Ok(Self {
            listen,
            journal_root,
            guest_root: GuestRoot(guest_root),
        })
    }
}

/// A required, non-blank value.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, HandsAgentConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(HandsAgentConfigError::Missing { name }),
    }
}

/// The first eight bytes of this binary's own `blake3`, stamped into every
/// response preamble so Brain can tell one agent build from another without
/// trusting the guest's own story about which it is.
fn agent_build() -> [u8; 8] {
    let hash = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| *blake3::hash(&bytes).as_bytes());
    let mut out = [0u8; 8];
    if let Some(hash) = hash {
        out.copy_from_slice(&hash[..8]);
    }
    out
}

/// Composes the guest.
///
/// # Errors
///
/// Returns [`HandsAgentRunError::Journal`] when the journal tree cannot be created.
pub fn compose(config: &Config) -> Result<Arc<serve::Guest>, HandsAgentRunError> {
    let journal = Journal::open(&config.journal_root)?;
    let executor = execute::Executor::new(Arc::new(host::HostRunner), config.guest_root.clone());
    Ok(Arc::new(serve::Guest::new(
        journal,
        executor,
        agent_build(),
        Arc::new(image::HostImageValidator::guest()),
    )))
}

/// Runs `hands-agent` until it stops.
///
/// # Errors
///
/// See [`HandsAgentRunError`].
pub async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), HandsAgentRunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_DEPLOYABLE,
            "hands-agent",
        ),
    );
    let guest = compose(config)?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| HandsAgentRunError::Listener {
            reason: format!("cannot bind {}: {error}", config.listen),
        })?;
    axum::serve(listener, serve::router(guest))
        .await
        .map_err(|error| HandsAgentRunError::Listener {
            reason: error.to_string(),
        })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("hands-agent: refusing to start: {error}");
            eprintln!(
                "hands-agent: required variables are {}",
                REQUIRED_VARS.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("hands-agent: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hands-agent: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, HandsAgentConfigError, agent_build, compose};
    use aex_hands_agent::boot::{
        GUEST_ROOT, GUEST_ROOT_VAR, JOURNAL_ROOT_VAR, LISTEN_ADDR, LISTEN_ADDR_VAR, REQUIRED_VARS,
    };
    use std::collections::BTreeMap;

    fn complete(journal: &str) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (LISTEN_ADDR_VAR, LISTEN_ADDR.to_owned()),
            (JOURNAL_ROOT_VAR, journal.to_owned()),
            (GUEST_ROOT_VAR, GUEST_ROOT.to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, HandsAgentConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn the_guest_configuration_is_three_values_and_none_of_them_is_defaulted() {
        let config = read(&complete("/var/lib/aex/hands")).expect("a complete environment");
        assert_eq!(config.listen.port(), 8080);
        assert!(config.listen.ip().is_unspecified());
        assert_eq!(config.guest_root.0, "/workspace");
        for name in REQUIRED_VARS {
            let mut vars = complete("/var/lib/aex/hands");
            vars.remove(name);
            assert_eq!(read(&vars), Err(HandsAgentConfigError::Missing { name }));
        }
    }

    #[test]
    fn the_guest_reads_no_aws_variable_at_all() {
        assert!(
            !REQUIRED_VARS.iter().any(|name| name.starts_with("AWS_")),
            "H-BOUNDARY B4: no credential has anywhere to arrive"
        );
        // A supplied AWS variable changes nothing, because nothing reads one.
        let mut vars = complete("/var/lib/aex/hands");
        vars.insert("AWS_ACCESS_KEY_ID", "AKIAPLANTED".to_owned());
        assert_eq!(
            read(&vars).expect("the environment is still complete"),
            read(&complete("/var/lib/aex/hands")).expect("the environment is complete")
        );
    }

    #[test]
    fn a_relative_guest_root_or_a_bad_listen_address_is_refused() {
        let mut relative = complete("/var/lib/aex/hands");
        relative.insert(GUEST_ROOT_VAR, "workspace".to_owned());
        assert!(matches!(
            read(&relative),
            Err(HandsAgentConfigError::Invalid {
                name: GUEST_ROOT_VAR,
                ..
            })
        ));

        let mut listen = complete("/var/lib/aex/hands");
        listen.insert(LISTEN_ADDR_VAR, "not-an-address".to_owned());
        assert!(matches!(
            read(&listen),
            Err(HandsAgentConfigError::Invalid {
                name: LISTEN_ADDR_VAR,
                ..
            })
        ));
    }

    #[test]
    fn composing_creates_the_journal_tree_and_starts_unbound() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("hands");
        let config = read(&complete(&root.to_string_lossy())).expect("a complete environment");
        let guest = compose(&config).expect("the journal tree is created");
        assert!(root.join("operations").is_dir());
        let unbound = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(guest.accepting());
        assert!(
            !unbound,
            "a guest with no binding accepts nothing, so there is no window where it is bound \
             but not replayed"
        );
    }

    #[test]
    fn the_agent_build_stamp_is_derived_from_the_binary_itself() {
        let stamp = agent_build();
        assert_eq!(stamp, agent_build(), "the stamp is stable within a build");
        assert_ne!(
            stamp, [0u8; 8],
            "a zero stamp would mean Brain cannot tell one agent build from another"
        );
    }
}

/// H-BOUNDARY B6, asserted against the binary that actually ships.
///
/// `crates/aex-hands-agent` already scans its own closure, but the rootfs
/// carries *this* package: the composition root, its HTTP server, its async
/// runtime and its syscall wrappers. A cloud dependency arriving through any of
/// those would not be caught there, so the scan is repeated here against the
/// artifact.
#[cfg(test)]
mod boundary {
    use std::collections::BTreeSet;

    /// Crate-name prefixes that indicate cloud authority of any kind.
    const FORBIDDEN_PREFIXES: [&str; 9] = [
        "aws-",
        "aws_",
        "rusoto",
        "azure_",
        "google-cloud",
        "gcp-",
        "lambda_runtime",
        "lambda_http",
        "aex-rds-data",
    ];

    /// Exact crate names that are forbidden even though their prefix is innocuous.
    const FORBIDDEN_EXACT: [&str; 4] = [
        "aex-secret-aws",
        "aex-content-aws",
        "aex-hands-control-aws",
        "aex-runtime-control-aws",
    ];

    /// The normal-and-build dependency closure of the shipped binary, by package name.
    fn closure() -> BTreeSet<String> {
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .exec()
            .expect("cargo metadata resolves the graph");
        let resolve = metadata
            .resolve
            .as_ref()
            .expect("a resolved graph is present");
        let root = metadata
            .packages
            .iter()
            .find(|package| package.name.as_str() == "hands-agent")
            .expect("this package is in the graph");

        let mut seen = BTreeSet::new();
        let mut frontier = vec![root.id.clone()];
        while let Some(id) = frontier.pop() {
            let Some(node) = resolve.nodes.iter().find(|node| node.id == id) else {
                continue;
            };
            for dependency in &node.deps {
                let is_normal = dependency.dep_kinds.iter().any(|kind| {
                    matches!(
                        kind.kind,
                        cargo_metadata::DependencyKind::Normal
                            | cargo_metadata::DependencyKind::Build
                    )
                });
                if !is_normal {
                    continue;
                }
                let name = metadata
                    .packages
                    .iter()
                    .find(|package| package.id == dependency.pkg)
                    .map(|package| package.name.to_string())
                    .unwrap_or_default();
                if seen.insert(name) {
                    frontier.push(dependency.pkg.clone());
                }
            }
        }
        seen
    }

    #[test]
    fn the_shipped_guest_binary_links_no_aws_sdk_or_credential_provider() {
        let closure = closure();
        assert!(
            !closure.is_empty(),
            "an empty closure would make this assertion vacuous"
        );
        let offenders: Vec<&String> = closure
            .iter()
            .filter(|name| {
                FORBIDDEN_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                    || FORBIDDEN_EXACT.contains(&name.as_str())
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "the guest binary must hold no cloud authority, but its closure contains {offenders:?}"
        );
    }

    #[test]
    fn the_binary_carries_no_tls_stack_it_could_reach_a_cloud_endpoint_with() {
        // Not a hygiene sweep: the workspace's pinned TLS backend is `aws-lc-rs`,
        // whose crate name the scan above already rejects. That is why workspace
        // materialize and persist — which need presigned HTTPS — are refused by this
        // guest and recorded as a gap rather than quietly linked in.
        let closure = closure();
        for client in ["reqwest", "rustls", "hyper-rustls", "native-tls"] {
            assert!(
                !closure.contains(client),
                "the guest links `{client}`, which is how a credential-free boundary stops being one"
            );
        }
    }
}
