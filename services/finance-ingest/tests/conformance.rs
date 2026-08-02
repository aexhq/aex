//! The event contract this deployable accepts, and what it refuses.

use aex_payment_contracts::{
    PinnedApiVersion, ProviderEventEnvelope, ProviderEventFacts, ProviderEventId,
    ProviderEventKind, ProviderObjectRef,
};
use aex_wire::PrefixedId as _;
use aex_wire::ids::{ContentHash, OrganizationId};
use aex_wire::types::{Cents, Timestamp};
use finance_ingest::handler::IngestRequest;
use finance_ingest::inbox::AppliedState;

/// The exact event set the webhook edge is configured to forward.
const FORWARDED: [ProviderEventKind; 5] = [
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
fn an_unmodelled_event_type_cannot_be_expressed_at_all() {
    let raw = r#"{"request":"provider_event","event":{"schemaVersion":1,
        "providerEventId":"evt_1","object":"ch_1","kind":"invoice_paid",
        "occurredAt":"2026-08-01T00:00:00.000Z","facts":{"kind":"invoice_paid"},
        "rawDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "providerApiVersion":"2026-06-24.dahlia","receivedAt":"2026-08-01T00:00:00.000Z"}}"#;
    assert!(
        serde_json::from_str::<IngestRequest>(raw).is_err(),
        "the closed event union admits no sixth type"
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
