//! The saved-card event boundary admits display metadata and no secret detail.

use aex_payment_contracts::{
    ProviderCustomerRef, ProviderEventFacts, ProviderEventKind, ProviderMethodRef,
};
use aex_wire::models::CardBrand;
use aex_wire::types::Timestamp;

fn created() -> Timestamp {
    Timestamp::from_unix_millis(1_800_000_000_000).expect("a fixture instant")
}

#[test]
fn attached_display_facts_have_the_exact_ingest_wire_shape() {
    let facts = ProviderEventFacts::PaymentMethodAttached {
        customer: ProviderCustomerRef("cus_fixture".to_owned()),
        method: ProviderMethodRef("pm_fixture".to_owned()),
        brand: CardBrand::Visa,
        last4: "4242".to_owned(),
        expiry_month: 12,
        expiry_year: 2032,
        provider_created_at: created(),
    };
    assert_eq!(facts.kind(), ProviderEventKind::PaymentMethodAttached);
    assert_eq!(facts.organization(), None, "ownership is database-resolved");
    assert_eq!(
        serde_json::to_value(&facts).expect("facts encode"),
        serde_json::json!({
            "kind": "payment_method_attached",
            "customer": "cus_fixture",
            "method": "pm_fixture",
            "brand": "visa",
            "last4": "4242",
            "expiryMonth": 12,
            "expiryYear": 2032,
            "providerCreatedAt": "2027-01-15T08:00:00.000Z"
        })
    );
}

#[test]
fn detached_facts_need_only_the_stable_provider_method_identity() {
    let facts = ProviderEventFacts::PaymentMethodDetached {
        method: ProviderMethodRef("pm_fixture".to_owned()),
    };
    assert_eq!(facts.kind(), ProviderEventKind::PaymentMethodDetached);
    assert_eq!(
        serde_json::to_value(facts).expect("facts encode"),
        serde_json::json!({"kind": "payment_method_detached", "method": "pm_fixture"})
    );
}

#[test]
fn secret_or_unbounded_card_members_are_rejected() {
    for forbidden in ["pan", "number", "cvc", "billingAddress", "raw"] {
        let mut value = serde_json::json!({
            "kind": "payment_method_attached",
            "customer": "cus_fixture",
            "method": "pm_fixture",
            "brand": "visa",
            "last4": "4242",
            "expiryMonth": 12,
            "expiryYear": 2032,
            "providerCreatedAt": "2027-01-15T08:00:00.000Z"
        });
        value
            .as_object_mut()
            .expect("an object")
            .insert(forbidden.to_owned(), serde_json::json!("forbidden"));
        assert!(
            serde_json::from_value::<ProviderEventFacts>(value).is_err(),
            "`{forbidden}` must not cross the event boundary"
        );
    }
}
