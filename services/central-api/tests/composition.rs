//! Exact session-MVP control surface and authority composition.

use aex_central_http::capability::Capability as _;
use aex_wire::routes::RouteId;
use aex_wire::server::{API_KEYS_ROUTES, AUTH_ROUTES, BILLING_ROUTES, BOOTSTRAP_ROUTES};
use central_api::{PERMISSIONS, manifest};

#[test]
fn the_public_control_surface_is_exactly_thirteen_routes() {
    assert_eq!(API_KEYS_ROUTES.len(), 3);
    assert_eq!(AUTH_ROUTES.len(), 2);
    assert_eq!(BILLING_ROUTES.len(), 7);
    assert_eq!(BOOTSTRAP_ROUTES, &[RouteId::DashboardBootstrapGet]);
    assert_eq!(
        API_KEYS_ROUTES.len() + AUTH_ROUTES.len() + BILLING_ROUTES.len() + BOOTSTRAP_ROUTES.len(),
        13
    );
}

#[test]
fn billing_keeps_only_the_essential_prepaid_surface() {
    assert_eq!(
        BILLING_ROUTES,
        &[
            RouteId::BillingBalanceGet,
            RouteId::BillingPaymentMethodDelete,
            RouteId::BillingPaymentMethodSessionCreate,
            RouteId::BillingPaymentMethodsList,
            RouteId::BillingTopUpCheckoutCreate,
            RouteId::BillingTransactionsList,
            RouteId::BillingUsageGet,
        ]
    );
}

#[test]
fn every_declared_capability_is_bound_and_no_retired_queue_right_survives() {
    let manifest = manifest();
    for capability in &manifest.capabilities {
        assert!(
            manifest
                .bindings
                .iter()
                .any(|binding| binding.capability == *capability),
            "{capability} has no resource binding"
        );
    }
    for forbidden in ["kms:", "sqs:"] {
        assert!(
            PERMISSIONS
                .iter()
                .all(|permission| !permission.starts_with(forbidden))
        );
    }
    assert!(
        !manifest
            .capabilities
            .contains(aex_central_http::capability::ControlQueueConsume::ID)
    );
}

#[test]
fn retired_control_and_billing_features_are_absent_from_the_handlers() {
    let control = include_str!("../src/control.rs");
    let billing = include_str!("../src/billing/service.rs");
    for retired in ["invitation", "workspace_delete", "organization_create"] {
        assert!(
            !control.contains(retired),
            "retired control feature `{retired}` survived"
        );
    }
    for retired in ["auto_topup", "statement_download", "portal_session"] {
        assert!(
            !billing.contains(retired),
            "retired billing feature `{retired}` survived"
        );
    }
}
