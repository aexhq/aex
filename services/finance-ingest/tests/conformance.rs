//! The event contract this deployable accepts, and what it refuses.

use aex_payment_contracts::{
    PinnedApiVersion, ProviderCustomerRef, ProviderEventEnvelope, ProviderEventFacts,
    ProviderEventId, ProviderEventKind, ProviderMethodRef, ProviderObjectRef,
};
use aex_wire::PrefixedId as _;
use aex_wire::ids::{ContentHash, OrganizationId};
use aex_wire::models::CardBrand;
use aex_wire::types::{Cents, Timestamp};
use finance_ingest::handler::IngestRequest;
use finance_ingest::inbox::AppliedState;

/// The exact event set the webhook edge is configured to forward.
const FORWARDED: [ProviderEventKind; 8] = [
    ProviderEventKind::PaymentMethodAttached,
    ProviderEventKind::PaymentMethodUpdated,
    ProviderEventKind::PaymentMethodDetached,
    ProviderEventKind::PaymentIntentSucceeded,
    ProviderEventKind::PaymentIntentPaymentFailed,
    ProviderEventKind::ChargeDisputeCreated,
    ProviderEventKind::RefundCreated,
    ProviderEventKind::RefundFailed,
];

fn organization() -> OrganizationId {
    OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("an organization")
}

fn envelope(facts: ProviderEventFacts) -> ProviderEventEnvelope {
    ProviderEventEnvelope {
        schema_version: aex_internal_contracts::SchemaVersion::V1,
        provider_event_id: ProviderEventId("evt_abcdefghij".to_owned()),
        object: ProviderObjectRef("pi_abcdefghij".to_owned()),
        kind: facts.kind(),
        occurred_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
        facts,
        raw_digest: ContentHash::from_bytes([5u8; 32]),
        provider_api_version: PinnedApiVersion("2026-06-24.dahlia".to_owned()),
        effect: None,
        received_at: Timestamp::from_unix_millis(1_800_000_000_001).expect("an instant"),
    }
}

#[test]
fn every_forwarded_event_kind_round_trips_through_the_invoke_payload() {
    for kind in FORWARDED {
        let facts = match kind {
            ProviderEventKind::PaymentMethodAttached => ProviderEventFacts::PaymentMethodAttached {
                customer: ProviderCustomerRef("cus_abcdefghij".to_owned()),
                method: ProviderMethodRef("pm_abcdefghij".to_owned()),
                brand: CardBrand::Visa,
                last4: "4242".to_owned(),
                expiry_month: 12,
                expiry_year: 2032,
                provider_created_at: Timestamp::from_unix_millis(1_799_999_000_000)
                    .expect("an instant"),
            },
            ProviderEventKind::PaymentMethodUpdated => ProviderEventFacts::PaymentMethodUpdated {
                customer: ProviderCustomerRef("cus_abcdefghij".to_owned()),
                method: ProviderMethodRef("pm_abcdefghij".to_owned()),
                brand: CardBrand::Mastercard,
                last4: "4444".to_owned(),
                expiry_month: 11,
                expiry_year: 2034,
                provider_created_at: Timestamp::from_unix_millis(1_799_999_000_000)
                    .expect("an instant"),
            },
            ProviderEventKind::PaymentMethodDetached => ProviderEventFacts::PaymentMethodDetached {
                method: ProviderMethodRef("pm_abcdefghij".to_owned()),
            },
            ProviderEventKind::PaymentIntentSucceeded => {
                ProviderEventFacts::PaymentIntentSucceeded {
                    organization: organization(),
                    credit: Cents::new(1_000),
                    charged: Cents::new(1_000),
                    intent: ProviderObjectRef("pi_abcdefghij".to_owned()),
                }
            }
            ProviderEventKind::PaymentIntentPaymentFailed => {
                ProviderEventFacts::PaymentIntentPaymentFailed {
                    organization: organization(),
                    intent: ProviderObjectRef("pi_abcdefghij".to_owned()),
                    failure: aex_payment_contracts::PaymentFailure {
                        class: aex_payment_contracts::PaymentFailureClass::CardDeclined,
                        provider_code: None,
                        decline_code: None,
                        retryable: false,
                    },
                }
            }
            ProviderEventKind::ChargeDisputeCreated => ProviderEventFacts::ChargeDisputeCreated {
                organization: organization(),
                charge: ProviderObjectRef("ch_abcdefghij".to_owned()),
                amount: Cents::new(1_000),
            },
            ProviderEventKind::RefundCreated => ProviderEventFacts::RefundCreated {
                organization: organization(),
                charge: ProviderObjectRef("ch_abcdefghij".to_owned()),
                refund: ProviderObjectRef("re_abcdefghij".to_owned()),
                amount: Cents::new(400),
            },
            ProviderEventKind::RefundFailed => ProviderEventFacts::RefundFailed {
                organization: organization(),
                charge: ProviderObjectRef("ch_abcdefghij".to_owned()),
                refund: ProviderObjectRef("re_abcdefghij".to_owned()),
                amount: Cents::new(400),
            },
        };
        let request = IngestRequest::ProviderEvent {
            event: Box::new(envelope(facts)),
        };
        let encoded = serde_json::to_string(&request).expect("the payload encodes");
        let decoded: IngestRequest = serde_json::from_str(&encoded).expect("the payload decodes");
        assert_eq!(decoded, request, "{kind:?} does not round-trip");
    }
}

#[test]
fn provider_endpoint_policy_is_the_same_exact_closed_event_set() {
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("../../../release/stripe-endpoint.json"))
            .expect("the endpoint policy decodes");
    let actual: Vec<&str> = policy["enabledEvents"]
        .as_array()
        .expect("enabledEvents is an array")
        .iter()
        .map(|value| value.as_str().expect("an event type"))
        .collect();
    assert_eq!(
        actual,
        vec![
            "payment_method.attached",
            "payment_method.updated",
            "payment_method.detached",
            "payment_intent.succeeded",
            "payment_intent.payment_failed",
            "charge.dispute.created",
            "refund.created",
            "refund.failed",
        ]
    );
    assert_eq!(actual.len(), FORWARDED.len());
}

#[test]
fn the_typescript_edge_fixture_decodes_as_the_exact_rust_invoke_request() {
    let raw =
        include_str!("../../stripe-webhook-edge/test/fixtures/payment-method-attached-ingest.json");
    let request: IngestRequest = serde_json::from_str(raw).expect("the edge fixture decodes");
    let IngestRequest::ProviderEvent { event } = request else {
        panic!("the fixture must invoke a provider event")
    };
    assert_eq!(event.kind, ProviderEventKind::PaymentMethodAttached);
    assert_eq!(event.facts.kind(), event.kind);
    assert_eq!(event.facts.organization(), None);

    let money_raw = include_str!(
        "../../stripe-webhook-edge/test/fixtures/payment-intent-succeeded-ingest.json"
    );
    let money: IngestRequest = serde_json::from_str(money_raw).expect("the money fixture decodes");
    let IngestRequest::ProviderEvent { event } = money else {
        panic!("the fixture must invoke a provider event")
    };
    assert_eq!(event.kind, ProviderEventKind::PaymentIntentSucceeded);
    assert_eq!(event.facts.organization(), Some(organization()));
}

#[test]
fn an_unmodelled_event_type_cannot_be_expressed_at_all() {
    let raw = r#"{"request":"provider_event","event":{"schemaVersion":1,
        "providerEventId":"evt_1","object":"ch_1","kind":"invoice_paid",
        "occurredAt":"2026-08-01T00:00:00.000Z","facts":{"kind":"invoice_paid"},
        "rawDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "providerApiVersion":"2026-06-24.dahlia","receivedAt":"2026-08-01T00:00:00.000Z"}}"#;
    assert!(
        serde_json::from_str::<IngestRequest>(raw).is_err(),
        "the closed event union admits no ninth type"
    );
}

#[test]
fn the_three_settled_states_have_exactly_the_durable_spellings_the_ddl_admits() {
    assert_eq!(AppliedState::Applied.as_str(), "applied");
    assert_eq!(
        AppliedState::IgnoredUnsupported.as_str(),
        "ignored_unsupported"
    );
    assert_eq!(AppliedState::Quarantined.as_str(), "quarantined");
    let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
    for state in [
        AppliedState::Applied,
        AppliedState::IgnoredUnsupported,
        AppliedState::Quarantined,
    ] {
        assert!(
            ddl.contains(&format!("'{}'", state.as_str())),
            "`{}` is not a declared applied_state",
            state.as_str()
        );
    }
}

#[test]
fn the_probe_arms_are_part_of_the_invoke_contract() {
    for raw in [r#"{"request":"healthz"}"#, r#"{"request":"readyz"}"#] {
        assert!(
            serde_json::from_str::<IngestRequest>(raw).is_ok(),
            "{raw} is a valid invocation"
        );
    }
}
