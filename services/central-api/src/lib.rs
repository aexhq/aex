//! `central-api`: the whole central HTTP surface in one long-lived task.
//!
//! # What this is
//!
//! One ECS Fargate service behind an Application Load Balancer, serving the
//! twenty-six mounted central operations that `central-control-api`,
//! `central-identity-api` and `finance-api` served as three Lambda deployables
//! behind an API Gateway. It is a **composition root only**: every handler, every
//! port and every adapter it uses belongs to one of those three crates, which is
//! why they now have library targets.
//!
//! # The one thing that is genuinely new
//!
//! Credential verification runs **in this process**. API Gateway cannot
//! integrate an ECS service, so the REQUEST authorizer that resolved a central
//! credential against Aurora is gone; behind a load balancer every header is
//! caller-authored, and a composition that still trusted an ambient context map
//! would admit whatever principal the caller asserted. So this root builds
//! [`aex_central_http::admission::CredentialAdmission`] over the Aurora login
//! and hands it to
//! [`aex_central_http::router::EdgeStack::with_in_process_authentication`],
//! which is exclusive with the gateway path by enum: a stack cannot both verify
//! and trust.
//!
//! # One Aurora login, as the three merged deployables already have
//!
//! One cluster, one login secret, one process. `central-control-api`,
//! `central-identity-api` and `finance-api` each connect through the cluster's
//! RDS-managed master secret, and this composition does the same, so the merge
//! changes nothing about how the database is reached.
//!
//! Per-schema `PostgreSQL` logins are a deferred capability, recorded in
//! `references/backlog.md`. They would defend service against service; they are
//! not what keeps one customer out of another's data. That is tenant isolation,
//! it lives in the application, and it is untouched by this.
//!
//! # What did not move
//!
//! `finance-ingest` stays on Lambda: `stripe-webhook-edge` reaches it by
//! `lambda:Invoke` on the webhook's synchronous path, a Fargate service cannot
//! be invoked that way, and `infra/modules/alb-service-target` refuses to
//! forward anything that is not under `/api/`. `central-authz`'s assertion
//! issuer — the half the five regional edges invoke — is untouched; only its
//! gateway-authorizer half becomes unnecessary.

pub mod config;

use std::collections::BTreeSet;
use std::sync::Arc;

use aex_central_http::capability::{
    AuthorizationRead, Capability as _, CapabilityBinding, CompositionError, CompositionManifest,
    ControlWrite, Declares, FinanceRead, IdentityWrite, PaymentCommandInvoke,
    RegionalControlInvoke, SignInHandshake, StatementRead,
};
use aex_central_http::health::{Dependency, Readiness};
use aex_central_http::router::{
    EdgeStack, mount_api_keys_api, mount_auth_api, mount_billing_api, mount_bootstrap_api,
    mount_central_operations_api, mount_identity_api, mount_organizations_api,
    mount_workspaces_api,
};
use aex_wire::server::{AuthApi, BillingApi};
use central_control_api::ControlApi;

pub use config::{Config, DEPLOYABLE};

/// This binary's capability declaration.
///
/// The union of what the three merged deployables declared, and nothing more.
/// A merge is the easiest place in a platform to acquire a right nobody asked
/// for, so the declaration is written out rather than derived: a binding for a
/// capability absent from this list is refused before any client is opened.
///
/// Notably absent: [`aex_central_http::capability::AssertionSign`]. This process
/// verifies credentials; it does not issue assertions. `central-authz` keeps
/// that key, and a plane that hands it here has widened the blast radius of the
/// one public listener to include minting regional authority.
#[allow(
    dead_code,
    reason = "the declaration is the capability list; its only use is the type-level `Declares` bound"
)]
struct Composition;

impl Declares<AuthorizationRead> for Composition {}
impl Declares<ControlWrite> for Composition {}
impl Declares<FinanceRead> for Composition {}
impl Declares<IdentityWrite> for Composition {}
impl Declares<PaymentCommandInvoke> for Composition {}
impl Declares<RegionalControlInvoke> for Composition {}
impl Declares<SignInHandshake> for Composition {}
impl Declares<StatementRead> for Composition {}

/// The manifest the start-up check runs against.
#[must_use]
pub fn manifest() -> CompositionManifest {
    CompositionManifest {
        deployable: DEPLOYABLE,
        capabilities: BTreeSet::from([
            AuthorizationRead::ID,
            ControlWrite::ID,
            FinanceRead::ID,
            IdentityWrite::ID,
            PaymentCommandInvoke::ID,
            RegionalControlInvoke::ID,
            SignInHandshake::ID,
            StatementRead::ID,
        ]),
        bindings: vec![
            // The cluster is the address and the secret is the login, and one
            // login reaches every central schema, so the secret is bound to each
            // of the four database capabilities rather than to one. Naming them
            // separately still keeps the list honest about what this deployable
            // may do, and both are ARN bindings, so a cluster or a secret from
            // another account or region is refused before a client is opened.
            CapabilityBinding::arn(config::AURORA_CLUSTER_ARN, ControlWrite::ID),
            CapabilityBinding::arn(config::AURORA_SECRET_ARN, AuthorizationRead::ID),
            CapabilityBinding::arn(config::AURORA_SECRET_ARN, ControlWrite::ID),
            CapabilityBinding::arn(config::AURORA_SECRET_ARN, IdentityWrite::ID),
            CapabilityBinding::arn(config::AURORA_SECRET_ARN, FinanceRead::ID),
            CapabilityBinding::resource(config::API_KEY_PEPPER_SECRET_ID, ControlWrite::ID),
            CapabilityBinding::resource(config::IDENTITY_PEPPER_SECRET_ID, IdentityWrite::ID),
            // Google's registered OAuth client. Bound to
            // `SignInHandshake` rather than to `IdentityWrite`, because holding a
            // credential that speaks as AEX at somebody else's authority is a
            // different right from writing this platform's own identity rows.
            CapabilityBinding::resource(config::GOOGLE_OAUTH_SECRET_ID, SignInHandshake::ID),
            CapabilityBinding::resource(config::REGIONAL_FUNCTION_ARNS, RegionalControlInvoke::ID),
            CapabilityBinding::arn(config::STRIPE_COMMAND_EDGE_ARN, PaymentCommandInvoke::ID),
            CapabilityBinding::resource(config::STATEMENT_BUCKET, StatementRead::ID),
        ],
    }
}

/// The IAM permissions this deployable requires, as a reviewable list.
///
/// The union of the three merged task roles. `kms:*` is absent and stays
/// absent: no central route decrypts anything, and the statement bucket is read
/// through a presigned URL the caller redeems, not by this task.
pub const PERMISSIONS: &[&str] = &[
    "rds-data:BeginTransaction",
    "rds-data:CommitTransaction",
    "rds-data:ExecuteStatement",
    "rds-data:RollbackTransaction",
    "secretsmanager:GetSecretValue",
    "lambda:InvokeFunction",
    "s3:GetObject",
];

/// What each start-up probe answered.
///
/// One field per probe rather than one boolean for "everything": a process that
/// reports "not ready" without naming which authority did not answer is a page
/// nobody can action, and this composition has six of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per start-up probe; a bitfield would hide which probe failed"
)]
pub struct Probes {
    /// `SELECT 1` on the Aurora login succeeded.
    pub aurora: bool,
    /// That same login proved the finance grants.
    pub finance: bool,
    /// The API-key pepper loaded.
    pub api_key_pepper: bool,
    /// The identity pepper loaded.
    pub identity_pepper: bool,
    /// The cursor signing secret loaded.
    pub cursor_secret: bool,
    /// The direct-invoke map covers every configured region.
    pub endpoints: bool,
}

impl Probes {
    /// No probe has answered yet.
    pub const NONE: Self = Self {
        aurora: false,
        finance: false,
        api_key_pepper: false,
        identity_pepper: false,
        cursor_secret: false,
        endpoints: false,
    };

    /// Every required authority answered its real probe.
    pub const READY: Self = Self {
        aurora: true,
        finance: true,
        api_key_pepper: true,
        identity_pepper: true,
        cursor_secret: true,
        endpoints: true,
    };
}

/// The readiness projection.
#[must_use]
pub fn readiness(probes: Probes) -> Readiness {
    Readiness::new(
        DEPLOYABLE.as_str(),
        vec![
            Dependency {
                name: "aurora",
                resolved: probes.aurora,
            },
            Dependency {
                name: "aurora-finance",
                resolved: probes.finance,
            },
            Dependency {
                name: "api-key-pepper",
                resolved: probes.api_key_pepper,
            },
            Dependency {
                name: "identity-pepper",
                resolved: probes.identity_pepper,
            },
            Dependency {
                name: "cursor-secret",
                resolved: probes.cursor_secret,
            },
            Dependency {
                name: "regional-endpoint-map",
                resolved: probes.endpoints,
            },
        ],
    )
}

/// Why `central-api` stopped.
#[derive(Debug, thiserror::Error)]
pub enum CentralApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] config::CentralApiConfigError),
    /// The composition was refused before any client was opened.
    #[error(transparent)]
    Composition(#[from] CompositionError),
    /// A required authority did not answer its start-up probe.
    #[error("dependency `{0}` refused startup: {1}")]
    Dependency(&'static str, String),
    /// The listener could not be bound, served or drained.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

/// Builds the router this service serves.
///
/// Each group is mounted by iterating its generated route slice, so the mounted
/// set is `CentralServiceId::CentralApi.routes()` by construction and a route
/// the contract adds is mounted without a source change here.
///
/// The three service implementations are separate arguments rather than one
/// bound, because they are genuinely three authorities over three schemas: a
/// single bound would let a composition satisfy the billing half with the
/// control store.
pub fn app<C, A, B, I>(
    control: Arc<C>,
    auth: Arc<A>,
    billing: Arc<B>,
    account: Arc<I>,
    edge: EdgeStack,
    readiness: Readiness,
) -> axum::Router
where
    C: ControlApi,
    A: AuthApi,
    B: BillingApi,
    I: aex_wire::server::IdentityApi,
{
    aex_central_http::health::router(readiness)
        .merge(mount_api_keys_api(Arc::clone(&control), edge.clone()))
        .merge(mount_bootstrap_api(Arc::clone(&control), edge.clone()))
        .merge(mount_central_operations_api(
            Arc::clone(&control),
            edge.clone(),
        ))
        .merge(mount_organizations_api(Arc::clone(&control), edge.clone()))
        .merge(mount_workspaces_api(control, edge.clone()))
        .merge(mount_auth_api(auth, edge.clone()))
        .merge(mount_identity_api(account, edge.clone()))
        .merge(mount_billing_api(billing, edge))
        .merge(aex_central_http::mount_deferred(DEPLOYABLE))
}

#[cfg(test)]
mod tests {
    use super::{DEPLOYABLE, PERMISSIONS, Probes, manifest, readiness};
    use aex_central_http::capability::{
        AssertionSign, Capability as _, CapabilityBinding, CompositionError, ControlQueueConsume,
    };
    use aex_central_http::config::CentralServiceId;

    #[test]
    fn the_merged_deployable_declares_every_group_it_must_serve() {
        assert_eq!(DEPLOYABLE, CentralServiceId::CentralApi);
        assert_eq!(DEPLOYABLE.groups().len(), 8);
        assert_eq!(DEPLOYABLE.routes().len(), 31);
    }

    #[test]
    fn the_readiness_projection_names_one_dependency_per_probe() {
        assert!(!readiness(Probes::NONE).is_ready());
        assert!(readiness(Probes::READY).is_ready());
        assert_eq!(readiness(Probes::NONE).unresolved().len(), 6);
        assert!(readiness(Probes::READY).unresolved().is_empty());
    }

    #[test]
    fn a_single_unanswered_probe_keeps_the_whole_process_out_of_rotation() {
        let mut probes = Probes::READY;
        probes.finance = false;
        let readiness = readiness(probes);
        assert!(!readiness.is_ready());
        assert_eq!(readiness.unresolved(), vec!["aurora-finance"]);
    }

    #[test]
    fn every_declared_capability_is_bound_and_every_binding_is_declared() {
        // `admit` is exercised against a real configuration by the integration
        // suite. What is checked here is the manifest's internal agreement,
        // which needs no environment at all: a declared capability nothing
        // binds is a right nobody can trace to a resource, and a binding for an
        // undeclared capability is the widening this type exists to refuse.
        let manifest = manifest();
        for capability in &manifest.capabilities {
            assert!(
                manifest
                    .bindings
                    .iter()
                    .any(|binding| binding.capability == *capability),
                "`{capability}` is declared and bound to no resource"
            );
        }
        for binding in &manifest.bindings {
            assert!(
                manifest.capabilities.contains(binding.capability),
                "`{}` binds the undeclared capability `{}`",
                binding.key,
                binding.capability
            );
        }
    }

    /// A resolution that satisfies every binding this manifest declares.
    ///
    /// Built from the manifest rather than from a hand-listed map, so a binding
    /// added without a value cannot make the refusal tests below pass for the
    /// wrong reason.
    fn resolved() -> aex_central_http::capability::ResolvedConfig {
        use aex_central_http::config::DeploymentPlane;
        use aex_wire::types::Region;

        let region = Region::EuWest1;
        let account = "000000000000";
        aex_central_http::capability::ResolvedConfig {
            deployable: DEPLOYABLE.as_str().to_owned(),
            plane: DeploymentPlane::Dev,
            region,
            account_id: account.to_owned(),
            values: manifest()
                .bindings
                .iter()
                .map(|binding| {
                    let value = if binding.arn {
                        format!("arn:aws:service:{}:{account}:resource", region.as_str())
                    } else {
                        "resource".to_owned()
                    };
                    (binding.key.to_owned(), value)
                })
                .collect(),
        }
    }

    #[test]
    fn this_composition_cannot_link_assertion_signing_or_a_queue() {
        // The two rights a merged public listener must never acquire: minting
        // regional authority, and draining the control queue the worker owns.
        for capability in [AssertionSign::ID, ControlQueueConsume::ID] {
            let manifest = manifest();
            assert!(
                !manifest.capabilities.contains(capability),
                "`{capability}` must not be declared by a public central listener"
            );
            let mut widened = manifest;
            widened
                .bindings
                .push(CapabilityBinding::resource("AEX_CENTRAL_API_X", capability));
            assert_eq!(
                aex_central_http::capability::admit(&widened, &resolved()),
                Err(CompositionError::ForbiddenCapability {
                    key: "AEX_CENTRAL_API_X",
                    capability
                })
            );
        }
    }

    #[test]
    fn the_declared_manifest_admits_a_complete_resolution() {
        assert_eq!(
            aex_central_http::capability::admit(&manifest(), &resolved()),
            Ok(())
        );
    }

    #[test]
    fn the_permission_list_holds_no_key_management_or_queue_right() {
        for permission in PERMISSIONS {
            assert!(!permission.starts_with("kms:"), "{permission}");
            assert!(!permission.starts_with("sqs:"), "{permission}");
            assert!(!permission.contains("stripe"), "{permission}");
        }
        assert!(PERMISSIONS.contains(&"lambda:InvokeFunction"));
    }
}
