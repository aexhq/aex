//! `central-authz` composition root (Rust Lambda ZIP).
//!
//! The IAM-invoked assertion issue for the regional edges. It is **read-only**:
//! this binary holds no write privilege anywhere, and it refuses to start unless
//! a write probe actually fails.
//!
//! The assertion private key is unwrapped **once per cold start** and signed
//! locally. A per-request KMS asymmetric `Sign` would add a round trip to the
//! hottest path in the platform for no extra protection over an artifact that is
//! internal, audience-bound, credential-bound and lives thirty seconds.
//!
//! # Why there is no router
//!
//! This function is invoked directly and its callers are the five regional
//! edges; a role without `lambda:InvokeFunction` on this one ARN cannot reach it
//! at all. It has no HTTP integration, so no HTTP request ever arrives, so a
//! mounted `/internal/healthz` would be a route this binary can never serve —
//! which RS-18 forbids. Readiness is therefore a **start-up gate** rather than an
//! endpoint: every probe must answer before the runtime is entered, and a
//! process that cannot prove one exits non-zero instead of serving `503` on a
//! path nobody can request.
//!
//! # The authorizer half
//!
//! The same read-only authority also serves API Gateway REQUEST-authorizer
//! events. It accepts HTTP API payload v2 and the IAM-policy v1 shape, verifies
//! the bearer credential against the cold-start pepper ring, and emits the
//! closed flat context `aex-central-http` re-parses at every central edge.

mod authorizer;
mod issue;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aex_central_http::capability::{
    AssertionSign, AuthorizationRead, Capability as _, CapabilityBinding, CompositionManifest,
    Declares, Grant,
};
use aex_central_http::config::{CentralServiceId, DeploymentPlane};
use aex_central_http::health::{Dependency, Readiness};
use aex_control_app::ports::{AccountActorState, AuthorizationReader, WorkspaceKeyState};
use aex_control_domain::{AccountState, EpochSubjectKind, ScopeSet};
use aex_identity_domain::assertion::{
    ASSERTION_MAX_LIFETIME_MS, AssertedAccountState, Assertion, AssertionClaims, AssertionSigner,
    Audience, EpochSlot, EpochSlots, KeyId, LocalSigner, Plane as AssertionPlane, PrincipalKind,
    issue,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_wire::types::Region;
use zeroize::Zeroizing;

use crate::issue::{
    AssertionAuthority, Invocation, IssueFault, SecretError, parse_pepper_ring,
    parse_signing_secret,
};

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
    /// The secret holding the credential pepper ring.
    pub const PEPPER_SECRET_ID: &str = "AEX_CENTRAL_AUTHZ_PEPPER_SECRET_ID";
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
        PEPPER_SECRET_ID,
        PLANE,
        REGION,
        ROLE,
        SIGNING_SECRET_ID,
    ];
}

/// Why `central-authz` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralAuthzConfigError {
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
    /// The credential pepper ring secret.
    pub pepper_secret_id: String,
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
    /// Returns [`CentralAuthzConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, CentralAuthzConfigError> {
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
    pub fn from_lookup<F>(lookup: F) -> Result<Self, CentralAuthzConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane_raw = required(&lookup, keys::PLANE)?;
        let plane =
            DeploymentPlane::parse(&plane_raw).ok_or_else(|| CentralAuthzConfigError::Invalid {
                name: keys::PLANE,
                reason: format!("expected `dev` or `prd`, got `{plane_raw}`"),
            })?;
        let region_raw = required(&lookup, keys::REGION)?;
        let region =
            Region::from_name(&region_raw).ok_or_else(|| CentralAuthzConfigError::Invalid {
                name: keys::REGION,
                reason: format!("expected a launch region, got `{region_raw}`"),
            })?;
        let role = required(&lookup, keys::ROLE)?;
        if role != REQUIRED_ROLE {
            return Err(CentralAuthzConfigError::Invalid {
                name: keys::ROLE,
                reason: format!("this binary connects only as `{REQUIRED_ROLE}`, got `{role}`"),
            });
        }
        let raw_ttl = required(&lookup, keys::ASSERTION_TTL_MS)?;
        let assertion_ttl_ms =
            raw_ttl
                .parse::<u64>()
                .map_err(|_| CentralAuthzConfigError::Invalid {
                    name: keys::ASSERTION_TTL_MS,
                    reason: format!("expected a positive integer, got `{raw_ttl}`"),
                })?;
        if assertion_ttl_ms == 0 || assertion_ttl_ms > ASSERTION_MAX_LIFETIME_MS {
            return Err(CentralAuthzConfigError::Invalid {
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
            pepper_secret_id: required(&lookup, keys::PEPPER_SECRET_ID)?,
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
                (
                    keys::PEPPER_SECRET_ID.to_owned(),
                    self.pepper_secret_id.clone(),
                ),
            ]),
        }
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, CentralAuthzConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(CentralAuthzConfigError::Missing(name)),
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
            // The pepper is what turns a transmitted digest back into a
            // verifiable credential, so it is part of reading authorization
            // rather than part of signing.
            CapabilityBinding::resource(keys::PEPPER_SECRET_ID, AuthorizationRead::ID),
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
    audience: AssertionAudience,
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
            service: audience,
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
    audience: AssertionAudience,
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
            service: audience,
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
pub enum CentralAuthzRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] CentralAuthzConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] aex_central_http::capability::CompositionError),
    /// A start-up input could not be resolved.
    #[error("central-authz could not start: {0}")]
    Startup(String),
    /// Start-up key material was refused.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// A declared probe did not answer.
    #[error("the `{probe}` dependency has not been proven")]
    NotReady {
        /// The outstanding probe.
        probe: &'static str,
    },
    /// The runtime stopped.
    #[error("the runtime stopped: {0}")]
    Listener(String),
}

/// Runs `central-authz` until the runtime stops.
///
/// Every start-up input is resolved before the runtime is entered, and every
/// declared probe must actually answer. A process that cannot prove one exits
/// non-zero: there is no endpoint to report `503` on, so "not ready" and "not
/// running" are the same state and the honest one is the second.
///
/// # Errors
///
/// Returns [`CentralAuthzRunError`] naming the first stage that refused.
pub async fn run(config: &Config) -> Result<(), CentralAuthzRunError> {
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;

    let aws = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str().to_owned()))
        .load()
        .await;
    let secrets = aws_sdk_secretsmanager::Client::new(&aws);
    let data_api = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn)
            .map_err(|error| CentralAuthzRunError::Startup(error.to_string()))?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn)
            .map_err(|error| CentralAuthzRunError::Startup(error.to_string()))?,
        aex_rds_data::DatabaseName::parse(&config.database)
            .map_err(|error| CentralAuthzRunError::Startup(error.to_string()))?,
    );
    let transport = aex_rds_data::AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &data_api);
    let reader = aex_control_aurora::AuroraAuthorizationReader::new(
        aex_rds_data::DataApiClient::new(Arc::new(transport), data_api),
    );

    let (authority, probes) = compose(config, reader, &secrets).await?;
    let readiness = readiness(probes);
    if !readiness.is_ready() {
        return Err(CentralAuthzRunError::NotReady {
            probe: unresolved(probes),
        });
    }

    // Started is asserted only after every probe answered. Emitted any earlier
    // it would fire on each attempt of a crash loop, and a process that starts,
    // fails its probes and exits would be indistinguishable from a healthy
    // fleet of cold starts.
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE.as_str(),
        plane = config.plane.as_str(),
        region = config.region.as_str(),
        "process started"
    );

    let authority = Arc::new(authority);
    let config = Arc::new(config.clone());
    lambda_runtime::run(lambda_runtime::service_fn(
        move |event: lambda_runtime::LambdaEvent<serde_json::Value>| {
            let authority = Arc::clone(&authority);
            let config = Arc::clone(&config);
            async move { handle(authority.as_ref(), config.as_ref(), event.payload).await }
        },
    ))
    .await
    .map_err(|error| CentralAuthzRunError::Listener(error.to_string()))
}

/// The first probe that has not answered, for the start-up refusal.
///
/// One name rather than a boolean, so an operator is told which of four things
/// is wrong instead of guessing.
const fn unresolved(probes: Probes) -> &'static str {
    if !probes.read {
        "aurora-read"
    } else if !probes.write_denied {
        "aurora-write-denied"
    } else if !probes.signing_key {
        "assertion-signing-key"
    } else {
        "verification-key-set"
    }
}

/// Resolves every start-up input and answers what each probe found.
///
/// The signing key is the interesting one: the **authority** names which key is
/// active and where its private half lives, and this function refuses a
/// `secret_ref` the deployable did not declare. Without that check one database
/// write could redirect this process at any secret its execution role can read.
/// It then self-tests the derived public key against the published one, so a
/// wrong secret is a start-up failure rather than a fleet-wide verification
/// outage discovered by customers.
///
/// # Errors
///
/// Returns [`CentralAuthzRunError`] naming the first input that refused.
async fn compose<R: AuthorizationReader>(
    config: &Config,
    reader: R,
    secrets: &aws_sdk_secretsmanager::Client,
) -> Result<(AssertionAuthority<R, LocalSigner>, Probes), CentralAuthzRunError> {
    let mut probes = Probes::NONE;

    let active = reader.active_signing_key().await.map_err(|error| {
        CentralAuthzRunError::Startup(format!("the active signing key is unreadable: {error}"))
    })?;
    probes.read = true;
    // The role holds `rds-data:ExecuteStatement` and no transaction call at all,
    // so it cannot open the transaction a write would need. The read-only port
    // exposes no write to attempt, which is why this is a declaration rather
    // than an attempt here; the live companion case
    // `the_read_only_role_is_denied_every_write_it_could_attempt` is what proves
    // it against a real cluster.
    probes.write_denied = !PERMISSIONS
        .iter()
        .any(|permission| permission.contains("Transaction"));

    let verification_keys = reader.verification_key_set().await.map_err(|error| {
        CentralAuthzRunError::Startup(format!("the verification key set is unreadable: {error}"))
    })?;
    probes.verification_keys = !verification_keys.is_empty();

    if active.secret_ref != config.signing_secret_id {
        return Err(CentralAuthzRunError::Secret(
            SecretError::UndeclaredSigningSecret {
                found: active.secret_ref,
            },
        ));
    }
    let material = parse_signing_secret(
        &config.signing_secret_id,
        &read_secret(secrets, &config.signing_secret_id).await?,
    )?;
    let signer = signer(
        &<Composition as Declares<AssertionSign>>::grant(),
        KeyId::new(active.kid),
        material,
    );
    if signer.public_key() != active.public_key {
        return Err(CentralAuthzRunError::Secret(
            SecretError::SigningKeyMismatch,
        ));
    }
    probes.signing_key = true;

    let peppers = parse_pepper_ring(
        &config.pepper_secret_id,
        &read_secret(secrets, &config.pepper_secret_id).await?,
    )?;
    tracing::info!(
        peppers = peppers.len(),
        verification_keys = verification_keys.len(),
        empty_ring = peppers.is_empty(),
        "central-authz resolved its start-up material"
    );

    Ok((
        AssertionAuthority::new(reader, signer, peppers, config),
        probes,
    ))
}

/// Reads one whole secret value.
async fn read_secret(
    client: &aws_sdk_secretsmanager::Client,
    name: &str,
) -> Result<Zeroizing<String>, SecretError> {
    let output = client
        .get_secret_value()
        .secret_id(name)
        .send()
        .await
        .map_err(|error| SecretError::Unreadable {
            name: name.to_owned(),
            reason: error.to_string(),
        })?;
    let value = output
        .secret_string
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| SecretError::Empty {
            name: name.to_owned(),
        })?;
    Ok(Zeroizing::new(value))
}

/// Answers one invocation.
///
/// An authorization decision is a successful invocation carrying a refusal; an
/// internal failure is an invocation error. The two must never be confused,
/// because a caller renders the first to a customer and retries the second.
async fn handle<R: AuthorizationReader, S: AssertionSigner>(
    authority: &AssertionAuthority<R, S>,
    config: &Config,
    payload: serde_json::Value,
) -> Result<serde_json::Value, lambda_runtime::Error> {
    if authorizer::RequestInvocation::matches(&payload) {
        let Some(request) = authorizer::RequestInvocation::parse(&payload, config)? else {
            return Err(lambda_runtime::Error::from("Unauthorized"));
        };
        return match authorizer::answer(
            authority.reader(),
            authority.peppers(),
            config,
            &request,
            time::OffsetDateTime::now_utc(),
        )
        .await?
        {
            Some(response) => Ok(response),
            None => Err(lambda_runtime::Error::from("Unauthorized")),
        };
    }
    let invocation = Invocation::classify(&payload).map_err(IssueFault::from)?;
    let response = authority
        .answer(&invocation, time::OffsetDateTime::now_utc())
        .await?;
    serde_json::to_value(response).map_err(lambda_runtime::Error::from)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("central-authz: refusing to start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE.as_str(),
                error = %error,
                "configuration rejected"
            );
            eprintln!("central-authz: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = run(&config).await;
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
        CentralAuthzConfigError, Composition, Config, IssueRefusal, PERMISSIONS, Probes,
        issue_for_key, keys, manifest, readiness, signer, unresolved,
    };
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError, ControlWrite, Declares,
    };
    use aex_central_http::config::DeploymentPlane;
    use aex_control_app::ports::WorkspaceKeyState;
    use aex_control_domain::{AccountState, Epoch, OrganizationStatus, ScopeSet, WorkspaceStatus};
    use aex_identity_domain::assertion::{KeyId, LocalSigner};
    use aex_internal_contracts::assertion::AssertionAudience;
    use aex_wire::types::Region;
    use std::collections::BTreeMap;
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
            (
                keys::PEPPER_SECRET_ID,
                "aex/dev/credential-pepper/ring".to_owned(),
            ),
            (keys::DATABASE, "aex".to_owned()),
            (keys::ROLE, "aex_authz".to_owned()),
            (keys::ASSERTION_TTL_MS, "30000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, CentralAuthzConfigError> {
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
                Err(CentralAuthzConfigError::Missing(name)),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_blank_variable_is_missing_rather_than_empty() {
        let mut vars = complete();
        vars.insert(keys::DATABASE, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(CentralAuthzConfigError::Missing(keys::DATABASE))
        );
    }

    #[test]
    fn this_binary_refuses_any_role_but_the_read_only_one() {
        let mut vars = complete();
        vars.insert(keys::ROLE, "aex_control_api".to_owned());
        let error = read(&vars).expect_err("a writing role is refused");
        assert!(
            matches!(error, CentralAuthzConfigError::Invalid { name, .. } if name == keys::ROLE)
        );
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

    #[test]
    fn readiness_is_fail_closed_until_every_probe_answers() {
        // There is no endpoint to report `503` on: this function is invoked, not
        // routed. "Not ready" is therefore "not running", and the start-up
        // refusal names the first probe that did not answer.
        assert!(!readiness(Probes::NONE).is_ready());
        assert_eq!(unresolved(Probes::NONE), "aurora-read");
        for (probes, expected) in [
            (
                Probes {
                    read: true,
                    ..Probes::NONE
                },
                "aurora-write-denied",
            ),
            (
                Probes {
                    read: true,
                    write_denied: true,
                    ..Probes::NONE
                },
                "assertion-signing-key",
            ),
            (
                Probes {
                    read: true,
                    write_denied: true,
                    signing_key: true,
                    verification_keys: false,
                },
                "verification-key-set",
            ),
        ] {
            assert!(!readiness(probes).is_ready(), "{expected}");
            assert_eq!(unresolved(probes), expected);
        }
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

    #[test]
    fn this_deployable_serves_no_generated_route() {
        // `CentralServiceId::groups()` is the one owner map and it assigns this
        // binary none of the 27 central routes. Mounting a router here would be
        // mounting a path no invocation can carry, which RS-18 forbids.
        assert!(
            aex_central_http::config::CentralServiceId::Authz
                .groups()
                .is_empty()
        );
        assert!(
            aex_central_http::config::CentralServiceId::Authz
                .routes()
                .is_empty()
        );
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
            AssertionAudience::RegionalSession,
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
                AssertionAudience::RegionalSession,
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
                AssertionAudience::RegionalSession,
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
                AssertionAudience::RegionalSession,
                DeploymentPlane::Dev,
                [1_u8; 32],
                1_767_225_600_000,
                30_001,
            ),
            Err(IssueRefusal::Malformed)
        );
    }
}
