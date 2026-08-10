//! What `finance-api` mounts, and what it answers before a credential exists.
//!
//! The mount is asserted against the generated route table rather than a list
//! written here, so an authored billing route that is never mounted fails this
//! suite instead of returning a runtime `404`.

use std::sync::Arc;

use aex_wire::dispatch::RequestLimits;
use aex_wire::idempotency::IdempotencyKind;
use aex_wire::routes::{RouteId, route};
use aex_wire::server::RouteGroup;
use aex_wire::types::HttpMethod;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use finance_api::authority::{
    AuthorityError, BalanceRecord, BillingAuthority, EffectPreparation, GatewayError,
    PaymentGateway, PolicyChange, PolicyRecord, StatementHeader, StatementLineRecord,
    StatementPage,
};
use finance_api::billing::BillingService;
use finance_api::download::{DownloadError, PresignedObject, StatementDownloads};
use finance_api::edge::UnresolvedPrincipalEdge;
use finance_api::health::Readiness;
use finance_api::mount::{AppState, app, mounted_routes};
use tower::ServiceExt as _;

/// An authority that is never reached: every case here stops at the edge.
#[derive(Debug)]
struct UnreachableAuthority;

#[async_trait::async_trait]
impl BillingAuthority for UnreachableAuthority {
    async fn probe_role(&self) -> Result<(), AuthorityError> {
        Ok(())
    }

    async fn balance(
        &self,
        _organization: aex_wire::ids::OrganizationId,
    ) -> Result<BalanceRecord, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn account_profile(
        &self,
        _organization: aex_wire::ids::OrganizationId,
    ) -> Result<aex_control_domain::AccountProfile, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn policy(
        &self,
        _organization: aex_wire::ids::OrganizationId,
    ) -> Result<PolicyRecord, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn replace_policy(
        &self,
        _organization: aex_wire::ids::OrganizationId,
        _change: PolicyChange,
        _expect_revision: u64,
    ) -> Result<PolicyRecord, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn statements(
        &self,
        _organization: aex_wire::ids::OrganizationId,
        _before_period: Option<&str>,
        _limit: u32,
    ) -> Result<StatementPage, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn statement(
        &self,
        _organization: aex_wire::ids::OrganizationId,
        _statement_id: uuid::Uuid,
    ) -> Result<StatementHeader, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn statement_lines(
        &self,
        _organization: aex_wire::ids::OrganizationId,
        _period: &str,
    ) -> Result<Vec<StatementLineRecord>, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn prepare_effect(
        &self,
        _organization: aex_wire::ids::OrganizationId,
        _kind: aex_payment_contracts::CommandKind,
        _intent: &[u8],
        _amount: Option<aex_finance_domain::Microusd>,
        _deadline_millis: i64,
    ) -> Result<EffectPreparation, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn provider_customer(
        &self,
        _organization: aex_wire::ids::OrganizationId,
    ) -> Result<Option<aex_payment_contracts::ProviderCustomerRef>, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn billing_contact(
        &self,
        _organization: aex_wire::ids::OrganizationId,
    ) -> Result<aex_payment_contracts::RedactedEmail, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn settle_customer(
        &self,
        _effect: aex_payment_contracts::EffectId,
        _organization: aex_wire::ids::OrganizationId,
        _result: &aex_payment_contracts::PaymentResult,
    ) -> Result<Option<aex_payment_contracts::ProviderCustomerRef>, AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn bind_effect_command(
        &self,
        _effect: aex_payment_contracts::EffectId,
        _intent_hash: [u8; 32],
        _envelope: &aex_payment_contracts::PaymentCommandEnvelope,
    ) -> Result<(), AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }

    async fn finalize_effect(
        &self,
        _effect: aex_payment_contracts::EffectId,
        _result: &aex_payment_contracts::PaymentResult,
    ) -> Result<(), AuthorityError> {
        unreachable!("no case in this suite passes the edge")
    }
}

/// A gateway that is never reached.
#[derive(Debug)]
struct UnreachableGateway;

#[async_trait::async_trait]
impl PaymentGateway for UnreachableGateway {
    async fn execute(
        &self,
        _envelope: &aex_payment_contracts::PaymentCommandEnvelope,
    ) -> Result<aex_payment_contracts::PaymentResult, GatewayError> {
        unreachable!("no case in this suite passes the edge")
    }
}

/// A presigner that is never reached.
#[derive(Debug)]
struct UnreachableDownloads;

#[async_trait::async_trait]
impl StatementDownloads for UnreachableDownloads {
    async fn presign(
        &self,
        _object_key: &str,
        _ttl_millis: i64,
    ) -> Result<PresignedObject, DownloadError> {
        unreachable!("no case in this suite passes the edge")
    }
}

/// The application, with a readiness gate the caller chooses.
fn application(ready: bool) -> axum::Router {
    let readiness = Arc::new(Readiness::pending());
    if ready {
        readiness.hold();
    }
    app(AppState {
        edge: Arc::new(UnresolvedPrincipalEdge::new(RequestLimits::DEFAULT)),
        api: Arc::new(BillingService::new(
            Arc::new(UnreachableAuthority),
            Arc::new(UnreachableGateway),
            Arc::new(UnreachableDownloads),
            100,
            300_000,
            aex_wire::types::HttpsUrl::parse("https://aex.dev/dashboard/billing")
                .expect("a constant URL"),
        )),
        readiness,
    })
}

/// A concrete path for `template`, with every parameter bound.
fn concrete(template: &str) -> String {
    template
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                match segment {
                    "{organizationId}" => "org_01kyw2qa4pew48j2gb1g6gw3rg",
                    "{statementId}" => "stm_01kyw2qa4pew48j2gb1g6gw3rg",
                    other => unreachable!("the billing group binds no `{other}`"),
                }
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn every_generated_billing_route_is_reachable_rather_than_a_404() {
    for id in mounted_routes() {
        let descriptor = route(*id);
        let method = match descriptor.method {
            HttpMethod::Get => "GET",
            HttpMethod::Put => "PUT",
            HttpMethod::Post => "POST",
            HttpMethod::Delete => "DELETE",
        };
        let mut request = Request::builder()
            .method(method)
            .uri(concrete(descriptor.template));
        if descriptor.idempotency == IdempotencyKind::IdempotencyKey {
            request = request.header("idempotency-key", "idem_01kyw2qa4pew48j2gb1g6gw3rg");
        }
        let response = application(true)
            .oneshot(request.body(Body::from("{}")).expect("a valid request"))
            .await
            .expect("the router answers");
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{} is authored but not mounted",
            descriptor.operation_id
        );
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{} must fail closed until a credential is verified",
            descriptor.operation_id
        );
    }
}

#[tokio::test]
async fn a_route_this_deployable_does_not_own_is_not_mounted() {
    let response = application(true)
        .oneshot(
            Request::get("/api/organizations")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_replay_policy_is_enforced_from_the_table_before_the_credential() {
    // `billing_balance_get` declares no replay identity, so supplying one is a
    // refusal on its own terms rather than a silently ignored header.
    let response = application(true)
        .oneshot(
            Request::get("/api/billing/balance")
                .header("idempotency-key", "idem_01kyw2qa4pew48j2gb1g6gw3rg")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn health_and_readiness_answer_different_questions() {
    let healthy = application(false)
        .oneshot(
            Request::get("/internal/healthz")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(healthy.status(), StatusCode::OK);

    let unready = application(false)
        .oneshot(
            Request::get("/internal/readyz")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(unready.status(), StatusCode::SERVICE_UNAVAILABLE);

    let ready = application(true)
        .oneshot(
            Request::get("/internal/readyz")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(ready.status(), StatusCode::OK);
}

#[test]
fn the_mounted_set_is_the_generated_group_and_nothing_else() {
    assert_eq!(mounted_routes(), RouteGroup::Billing.routes());
    for id in RouteId::ALL {
        assert_eq!(
            mounted_routes().contains(id),
            RouteId::group(*id) == RouteGroup::Billing,
            "{} is on the wrong deployable",
            route(*id).operation_id
        );
    }
}
