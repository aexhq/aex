//! `central-authz` composition root (Rust Lambda ZIP).
//!
//! Two handlers in one binary: an API Gateway REQUEST authorizer for the central
//! plane, and an IAM-invoked assertion issue for the regional edges. Both are
//! **read-only**: this binary holds no write privilege anywhere, and its
//! readiness probe requires a write to fail.
//!
//! The assertion private key is unwrapped **once per cold start** and signed
//! locally. A per-request KMS asymmetric `Sign` would add a round trip to the
//! hottest path in the platform for no extra protection over an artifact that is
//! internal, audience-bound, credential-bound and lives thirty seconds.

use std::collections::{BTreeMap, BTreeSet};

use aex_central_http::capability::{
    AssertionSign, AuthorizationRead, Capability as _, CapabilityBinding, CompositionManifest,
    Declares, Grant,
};
use aex_central_http::config::{CentralServiceId, DeploymentPlane};
use aex_central_http::health::{Dependency, Readiness};
use aex_control_app::ports::{AccountActorState, WorkspaceKeyState};
use aex_control_domain::{AccountState, EpochSubjectKind, ScopeSet};
use aex_identity_domain::assertion::{
    ASSERTION_MAX_LIFETIME_MS, AssertedAccountState, Assertion, AssertionClaims, AssertionSigner,
    Audience, EpochSlot, EpochSlots, KeyId, LocalSigner, Plane as AssertionPlane, PrincipalKind,
    RegionalService, issue,
};
use aex_wire::types::Region;
use zeroize::Zeroizing;

/// The deployable this binary is.
const DEPLOYABLE: CentralServiceId = CentralServiceId::Authz;

/// The one login role this binary may connect as.
const REQUIRED_ROLE: &str = "aex_authz";

/// Environment keys, all inside the declared namespace.
mod keys {
    /// The plane's account id, for the ARN binding check.
    pub const ACCOUNT_ID: &str = "AEX_CENTRAL_AUTHZ_ACCOUNT_ID";
    /// The assertion lifetime, validated against the hard 30 s ceiling.
    pub const ASSERTION_TTL_MS: &str = "AEX_CENTRAL_AUTHZ_ASSERTION_TTL_MS";
    /// The Aurora cluster the read-only role connects to.
    pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_AUTHZ_AURORA_CLUSTER_ARN";
    /// The Secrets Manager secret holding the Aurora credentials.
    pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_AUTHZ_AURORA_SECRET_ARN";
    /// The database name.
    pub const DATABASE: &str = "AEX_CENTRAL_AUTHZ_DATABASE";
    /// The deployment plane.
    pub const PLANE: &str = "AEX_CENTRAL_AUTHZ_PLANE";
    /// The bound region.
    pub const REGION: &str = "AEX_CENTRAL_AUTHZ_REGION";
    /// The login role. Must be `aex_authz`.
    pub const ROLE: &str = "AEX_CENTRAL_AUTHZ_ROLE";
    /// The secret holding the wrapped assertion signing key.
    pub const SIGNING_SECRET_ID: &str = "AEX_CENTRAL_AUTHZ_SIGNING_SECRET_ID";

    /// Every key this binary reads, for the totality test.
    #[allow(
        dead_code,
        reason = "the inventory exists so the suite can remove each key in turn"
    )]
    pub const ALL: &[&str] = &[
        ACCOUNT_ID,
        ASSERTION_TTL_MS,
        AURORA_CLUSTER_ARN,
        AURORA_SECRET_ARN,
        DATABASE,
        PLANE,
        REGION,
        ROLE,
        SIGNING_SECRET_ID,
    ];
}

/// Why `central-authz` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A required variable was absent or blank.
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// Which variable.
        name: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// Validated start-up configuration.
///
/// Nothing here has a default. A defaulted resource identifier silently binds
/// the process to the wrong plane, region or cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Which deployment plane.
    pub plane: DeploymentPlane,
    /// Which region.
    pub region: Region,
    /// The plane's account id.
    pub account_id: String,
    /// The Aurora cluster.
    pub aurora_cluster_arn: String,
    /// The Aurora credentials secret.
    pub aurora_secret_arn: String,
    /// The wrapped signing-key secret.
    pub signing_secret_id: String,
    /// The database name.
    pub database: String,
    /// The login role.
    pub role: String,
    /// How long an issued assertion lives.
    pub assertion_ttl_ms: u64,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane_raw = required(&lookup, keys::PLANE)?;
        let plane = DeploymentPlane::parse(&plane_raw).ok_or_else(|| ConfigError::Invalid {
            name: keys::PLANE,
            reason: format!("expected `dev` or `prd`, got `{plane_raw}`"),
        })?;
        let region_raw = required(&lookup, keys::REGION)?;
        let region = Region::from_name(&region_raw).ok_or_else(|| ConfigError::Invalid {
            name: keys::REGION,
            reason: format!("expected a launch region, got `{region_raw}`"),
        })?;
        let role = required(&lookup, keys::ROLE)?;
        if role != REQUIRED_ROLE {
            return Err(ConfigError::Invalid {
                name: keys::ROLE,
                reason: format!("this binary connects only as `{REQUIRED_ROLE}`, got `{role}`"),
            });
        }
        let raw_ttl = required(&lookup, keys::ASSERTION_TTL_MS)?;
        let assertion_ttl_ms = raw_ttl.parse::<u64>().map_err(|_| ConfigError::Invalid {
            name: keys::ASSERTION_TTL_MS,
            reason: format!("expected a positive integer, got `{raw_ttl}`"),
        })?;
        if assertion_ttl_ms == 0 || assertion_ttl_ms > ASSERTION_MAX_LIFETIME_MS {
            return Err(ConfigError::Invalid {
                name: keys::ASSERTION_TTL_MS,
                reason: format!(
                    "expected 1..={ASSERTION_MAX_LIFETIME_MS}, got `{assertion_ttl_ms}`"
                ),
            });
        }
        Ok(Self {
            plane,
            region,
            account_id: required(&lookup, keys::ACCOUNT_ID)?,
            aurora_cluster_arn: required(&lookup, keys::AURORA_CLUSTER_ARN)?,
            aurora_secret_arn: required(&lookup, keys::AURORA_SECRET_ARN)?,
            signing_secret_id: required(&lookup, keys::SIGNING_SECRET_ID)?,
            database: required(&lookup, keys::DATABASE)?,
            role,
            assertion_ttl_ms,
        })
    }

    /// The resolved values the composition check runs over.
    #[must_use]
    pub fn resolved(&self) -> aex_central_http::capability::ResolvedConfig {
        aex_central_http::capability::ResolvedConfig {
            deployable: DEPLOYABLE.as_str().to_owned(),
            plane: self.plane,
            region: self.region,
            account_id: self.account_id.clone(),
            values: BTreeMap::from([
                (
                    keys::AURORA_CLUSTER_ARN.to_owned(),
                    self.aurora_cluster_arn.clone(),
                ),
                (
                    keys::SIGNING_SECRET_ID.to_owned(),
                    self.signing_secret_id.clone(),
                ),
            ]),
        }
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(name)),
    }
}

/// This binary's capability declaration.
///
/// Read-only everywhere plus one key unwrap. There is deliberately no write
/// capability, no queue, no object store and no regional invoke: `central-authz`
/// answers questions and signs one artifact, and a binding for anything else is
/// refused at start-up.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<AuthorizationRead> for Composition {}
impl Declares<AssertionSign> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([AuthorizationRead::ID, AssertionSign::ID]),
        bindings: vec![
            CapabilityBinding::arn(keys::AURORA_CLUSTER_ARN, AuthorizationRead::ID),
            CapabilityBinding::resource(keys::SIGNING_SECRET_ID, AssertionSign::ID),
        ],
    }
}

/// The IAM permissions this deployable requires, as a reviewable list.
///
/// `rds-data:ExecuteStatement` only: no `BeginTransaction`, no
/// `CommitTransaction`, no `RollbackTransaction`. A role that cannot open a
/// transaction cannot write even if a statement tried to.
pub const PERMISSIONS: &[&str] = &[
    "rds-data:ExecuteStatement",
    "secretsmanager:GetSecretValue",
    "kms:Decrypt",
];

/// What each start-up probe answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per start-up probe; a bitfield would hide which probe failed"
)]
pub struct Probes {
    /// `SELECT 1` as `aex_authz` succeeded.
    pub read: bool,
    /// The write probe **failed**, as a read-only role requires.
    pub write_denied: bool,
    /// The active signing key loaded and its self-test vector verified.
    pub signing_key: bool,
    /// The verification key set is non-empty.
    pub verification_keys: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        read: false,
        write_denied: false,
        signing_key: false,
        verification_keys: false,
    };
}

/// The readiness projection.
///
/// A role misconfiguration is a start-up failure rather than a runtime surprise,
/// which is why `write_denied` is a dependency like any other: a write that
/// **succeeds** leaves this binary not-ready.
#[must_use]
pub fn readiness(probes: Probes) -> Readiness {
    Readiness::new(
        DEPLOYABLE.as_str(),
        vec![
            Dependency {
                name: "aurora-read",
                resolved: probes.read,
            },
            Dependency {
                name: "aurora-write-denied",
                resolved: probes.write_denied,
            },
            Dependency {
                name: "assertion-signing-key",
                resolved: probes.signing_key,
            },
            Dependency {
                name: "verification-key-set",
                resolved: probes.verification_keys,
            },
        ],
    )
}

/// The assertion signer, unwrapped once per cold start.
///
/// The [`Grant`] argument is unforgeable outside this composition, so no library
/// can build a signer without being handed one here.
#[must_use]
pub fn signer(_grant: &Grant<AssertionSign>, kid: KeyId, secret: [u8; 32]) -> LocalSigner {
    LocalSigner::new(kid, &Zeroizing::new(secret))
}

/// The assertion plane this deployment plane addresses.
#[must_use]
pub const fn assertion_plane(plane: DeploymentPlane) -> AssertionPlane {
    match plane {
        DeploymentPlane::Dev => AssertionPlane::Dev,
        DeploymentPlane::Prd => AssertionPlane::Prd,
    }
}

/// Why an assertion could not be issued for a resolved credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IssueRefusal {
    /// The credential was revoked, lapsed, or its person disabled.
    #[error("the credential is not current")]
    NotCurrent,
    /// The account state could not be established.
    ///
    /// Never downgraded to "active": an unreadable state is a `503`, because
    /// asserting a state nobody could read is exactly how a paused account keeps
    /// spending.
    #[error("the account state is unavailable")]
    AccountStateUnavailable,
    /// The envelope refused the claims.
    #[error("the assertion could not be built")]
    Malformed,
}

/// The account state an envelope may carry.
const fn asserted_state(state: AccountState) -> Result<AssertedAccountState, IssueRefusal> {
    match state {
        AccountState::Active => Ok(AssertedAccountState::Active),
        AccountState::PausedTopUpRequired => Ok(AssertedAccountState::PausedTopUpRequired),
        AccountState::Unavailable => Err(IssueRefusal::AccountStateUnavailable),
    }
}

/// Issues an assertion for a resolved workspace key.
///
/// # Errors
///
/// Returns [`IssueRefusal`] naming the first rule that refused.
pub fn issue_for_key(
    signer: &dyn AssertionSigner,
    key: &WorkspaceKeyState,
    service: RegionalService,
    plane: DeploymentPlane,
    credential_binding: [u8; 32],
    now_ms: u64,
    ttl_ms: u64,
) -> Result<Assertion, IssueRefusal> {
    if key.key_revoked
        || key.workspace_status != aex_control_domain::WorkspaceStatus::Active
        || key.organization_status != aex_control_domain::OrganizationStatus::Active
    {
        return Err(IssueRefusal::NotCurrent);
    }
    let slots: Vec<EpochSlot> = key
        .epoch_subjects()
        .into_iter()
        .map(|(kind, id, epoch)| EpochSlot { kind, id, epoch })
        .collect();
    let claims = AssertionClaims {
        issued_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(ttl_ms),
        audience: Audience {
            plane: assertion_plane(plane),
            region: key.region,
            service,
        },
        principal_kind: PrincipalKind::WorkspaceKey,
        principal_id: key.key_id,
        credential_binding,
        organization_id: key.organization_id,
        workspace_id: key.workspace_id,
        workspace_region: key.region,
        account_state: asserted_state(key.account_state)?,
        // Effective scopes are computed here and never re-derived at the edge.
        scopes: key.scopes.intersect(ScopeSet::WORKSPACE_KEY_MINTABLE),
        epochs: EpochSlots::new(&slots).map_err(|_| IssueRefusal::Malformed)?,
    };
    issue(signer, &claims).map_err(|_| IssueRefusal::Malformed)
}

/// Issues an assertion for a resolved person, through either credential.
///
/// The same envelope for an account token and a browser session: same person,
/// same scopes, same role, different credential. A second format would be a
/// second thing for every regional edge to verify.
///
/// # Errors
///
/// Returns [`IssueRefusal`] naming the first rule that refused.
#[allow(
    clippy::too_many_arguments,
    reason = "every argument is a distinct claim the envelope binds; grouping them would only move the list"
)]
pub fn issue_for_actor(
    signer: &dyn AssertionSigner,
    actor: &AccountActorState,
    kind: PrincipalKind,
    service: RegionalService,
    plane: DeploymentPlane,
    credential_binding: [u8; 32],
    now_ms: u64,
    ttl_ms: u64,
) -> Result<Assertion, IssueRefusal> {
    if actor.credential_revoked || actor.credential_expired || !actor.user_active {
        return Err(IssueRefusal::NotCurrent);
    }
    let slots = [
        EpochSlot {
            kind: EpochSubjectKind::User,
            id: actor.user_id,
            epoch: actor.epoch_user,
        },
        EpochSlot {
            kind: EpochSubjectKind::Membership,
            id: actor.membership_id,
            epoch: actor.epoch_membership,
        },
        EpochSlot {
            kind: EpochSubjectKind::Workspace,
            id: actor.workspace_id,
            epoch: actor.epoch_workspace,
        },
        EpochSlot {
            kind: EpochSubjectKind::Account,
            id: actor.organization_id,
            epoch: actor.epoch_account,
        },
    ];
    let claims = AssertionClaims {
        issued_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(ttl_ms),
        audience: Audience {
            plane: assertion_plane(plane),
            region: actor.region,
            service,
        },
        principal_kind: kind,
        principal_id: actor.user_id,
        credential_binding,
        organization_id: actor.organization_id,
        workspace_id: actor.workspace_id,
        workspace_region: actor.region,
        account_state: asserted_state(actor.account_state)?,
        scopes: actor.scopes.intersect(actor.role.scopes()),
        epochs: EpochSlots::new(&slots).map_err(|_| IssueRefusal::Malformed)?,
    };
    issue(signer, &claims).map_err(|_| IssueRefusal::Malformed)
}

/// Why `central-authz` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] aex_central_http::capability::CompositionError),
    /// The listener stopped.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

/// Builds the router this binary serves.
///
/// The authorizer and the issue command are IAM-invoked rather than routed, so
/// the only mounted paths are the two internal probes.
pub fn app(readiness: Readiness) -> axum::Router {
    aex_central_http::health::router(readiness)
}

/// Runs `central-authz` until it stops.
///
/// # Errors
///
/// Returns [`RunError`] when configuration or composition is refused, or when
/// the listener stops.
pub async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), RunError> {
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );
    // Readiness stays fail-closed until each probe has actually answered. This
    // run is uncredentialed, so no probe can answer and the process serves `503`
    // on `/internal/readyz` rather than claiming a readiness it has not earned.
    lambda_http::run(app(readiness(Probes::NONE)))
        .await
        .map_err(|error| RunError::Listener(error.to_string()))
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("central-authz: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("central-authz: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-authz: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Composition, Config, ConfigError, IssueRefusal, PERMISSIONS, Probes, app, issue_for_key,
        keys, manifest, readiness, signer,
    };
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError, ControlWrite, Declares,
    };
    use aex_central_http::config::DeploymentPlane;
    use aex_central_http::health::{HEALTH_PATH, READY_PATH};
    use aex_control_app::ports::WorkspaceKeyState;
    use aex_control_domain::{AccountState, Epoch, OrganizationStatus, ScopeSet, WorkspaceStatus};
    use aex_identity_domain::assertion::{KeyId, LocalSigner, RegionalService};
    use aex_wire::types::Region;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::BTreeMap;
    use tower::ServiceExt as _;
    use uuid::Uuid;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (keys::PLANE, "dev".to_owned()),
            (keys::REGION, "eu-west-1".to_owned()),
            (keys::ACCOUNT_ID, "000000000000".to_owned()),
            (
                keys::AURORA_CLUSTER_ARN,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
            ),
            (
                keys::AURORA_SECRET_ARN,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-authz".to_owned(),
            ),
            (
                keys::SIGNING_SECRET_ID,
                "aex/dev/authz-signing/current".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::ROLE, "aex_authz".to_owned()),
            (keys::ASSERTION_TTL_MS, "30000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn a_complete_environment_is_accepted() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(config.plane, DeploymentPlane::Dev);
        assert_eq!(config.region, Region::EuWest1);
        assert_eq!(config.assertion_ttl_ms, 30_000);
    }

    #[test]
    fn every_variable_is_required_and_named_when_absent() {
        for name in keys::ALL {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(ConfigError::Missing(name)),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_blank_variable_is_missing_rather_than_empty() {
        let mut vars = complete();
        vars.insert(keys::DATABASE, "   ".to_owned());
        assert_eq!(read(&vars), Err(ConfigError::Missing(keys::DATABASE)));
    }

    #[test]
    fn this_binary_refuses_any_role_but_the_read_only_one() {
        let mut vars = complete();
        vars.insert(keys::ROLE, "aex_control_api".to_owned());
        let error = read(&vars).expect_err("a writing role is refused");
        assert!(matches!(error, ConfigError::Invalid { name, .. } if name == keys::ROLE));
    }

    #[test]
    fn an_assertion_lifetime_over_the_hard_ceiling_is_refused_at_start_up() {
        for ttl in ["0", "30001", "600000", "lots"] {
            let mut vars = complete();
            vars.insert(keys::ASSERTION_TTL_MS, ttl.to_owned());
            assert!(read(&vars).is_err(), "{ttl}");
        }
    }

    #[test]
    fn an_unknown_plane_or_region_is_refused() {
        let mut vars = complete();
        vars.insert(keys::PLANE, "staging".to_owned());
        assert!(read(&vars).is_err());
        let mut vars = complete();
        vars.insert(keys::REGION, "mars-central-1".to_owned());
        assert!(read(&vars).is_err());
    }

    #[test]
    fn the_composition_admits_exactly_its_declared_bindings() {
        let config = read(&complete()).expect("a complete environment");
        assert_eq!(
            aex_central_http::capability::admit(&manifest(), &config.resolved()),
            Ok(())
        );
    }

    #[test]
    fn this_binary_cannot_link_a_write_capability() {
        let config = read(&complete()).expect("a complete environment");
        let mut manifest = manifest();
        manifest.bindings.push(CapabilityBinding::arn(
            "AEX_CENTRAL_AUTHZ_CONTROL_QUEUE",
            ControlWrite::ID,
        ));
        assert_eq!(
            aex_central_http::capability::admit(&manifest, &config.resolved()),
            Err(CompositionError::ForbiddenCapability {
                key: "AEX_CENTRAL_AUTHZ_CONTROL_QUEUE",
                capability: ControlWrite::ID
            })
        );
    }

    #[test]
    fn an_off_plane_cluster_arn_is_refused_before_any_client_opens() {
        let mut vars = complete();
        vars.insert(
            keys::AURORA_CLUSTER_ARN,
            "arn:aws:rds:us-east-1:000000000000:cluster:aex".to_owned(),
        );
        let config = read(&vars).expect("a complete environment");
        assert_eq!(
            aex_central_http::capability::admit(&manifest(), &config.resolved()),
            Err(CompositionError::OffPlaneArn(keys::AURORA_CLUSTER_ARN))
        );
    }

    #[test]
    fn the_permission_list_is_read_only_and_holds_no_transaction_call() {
        for permission in PERMISSIONS {
            assert!(
                !permission.contains("BeginTransaction")
                    && !permission.contains("CommitTransaction")
                    && !permission.contains("RollbackTransaction"),
                "{permission}"
            );
        }
        assert!(PERMISSIONS.contains(&"rds-data:ExecuteStatement"));
        assert!(!PERMISSIONS.iter().any(|it| it.starts_with("sqs:")));
        assert!(!PERMISSIONS.iter().any(|it| it.starts_with("s3:")));
    }

    #[tokio::test]
    async fn readiness_is_fail_closed_until_every_probe_answers() {
        let response = app(readiness(Probes::NONE))
            .oneshot(
                Request::builder()
                    .uri(READY_PATH)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn a_write_that_succeeds_leaves_the_binary_not_ready() {
        let probes = Probes {
            read: true,
            write_denied: false,
            signing_key: true,
            verification_keys: true,
        };
        assert!(
            !readiness(probes).is_ready(),
            "a read-only role that can write is a start-up failure"
        );
        assert!(
            readiness(Probes {
                write_denied: true,
                ..probes
            })
            .is_ready()
        );
    }

    #[tokio::test]
    async fn liveness_answers_before_readiness_does() {
        let response = app(readiness(Probes::NONE))
            .oneshot(
                Request::builder()
                    .uri(HEALTH_PATH)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn key_state(state: AccountState) -> WorkspaceKeyState {
        WorkspaceKeyState {
            key_id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
            organization_id: Uuid::from_u128(3),
            scopes: ScopeSet::ALL,
            verifier: [0_u8; 32],
            pepper_version: 1,
            key_revoked: false,
            region: Region::EuWest1,
            workspace_status: WorkspaceStatus::Active,
            organization_status: OrganizationStatus::Active,
            account_state: state,
            epoch_key: Epoch::new(1),
            epoch_workspace: Epoch::new(1),
            epoch_account: Epoch::new(1),
        }
    }

    fn local_signer() -> LocalSigner {
        signer(
            &<Composition as Declares<AssertionSign>>::grant(),
            KeyId::new(Uuid::from_u128(9)),
            [7_u8; 32],
        )
    }

    #[test]
    fn an_issued_key_assertion_is_the_fixed_length_envelope() {
        let assertion = issue_for_key(
            &local_signer(),
            &key_state(AccountState::Active),
            RegionalService::SessionApi,
            DeploymentPlane::Dev,
            [1_u8; 32],
            1_767_225_600_000,
            30_000,
        )
        .expect("a current key issues");
        assert_eq!(
            assertion.as_bytes().len(),
            aex_identity_domain::assertion::ASSERTION_ENVELOPE_LEN
        );
    }

    #[test]
    fn an_unreadable_account_state_never_becomes_an_active_assertion() {
        assert_eq!(
            issue_for_key(
                &local_signer(),
                &key_state(AccountState::Unavailable),
                RegionalService::SessionApi,
                DeploymentPlane::Dev,
                [1_u8; 32],
                1_767_225_600_000,
                30_000,
            ),
            Err(IssueRefusal::AccountStateUnavailable)
        );
    }

    #[test]
    fn a_revoked_key_never_issues() {
        let mut key = key_state(AccountState::Active);
        key.key_revoked = true;
        assert_eq!(
            issue_for_key(
                &local_signer(),
                &key,
                RegionalService::SessionApi,
                DeploymentPlane::Dev,
                [1_u8; 32],
                1_767_225_600_000,
                30_000,
            ),
            Err(IssueRefusal::NotCurrent)
        );
    }

    #[test]
    fn a_lifetime_over_the_ceiling_is_refused_by_the_envelope_itself() {
        assert_eq!(
            issue_for_key(
                &local_signer(),
                &key_state(AccountState::Active),
                RegionalService::SessionApi,
                DeploymentPlane::Dev,
                [1_u8; 32],
                1_767_225_600_000,
                30_001,
            ),
            Err(IssueRefusal::Malformed)
        );
    }
}
