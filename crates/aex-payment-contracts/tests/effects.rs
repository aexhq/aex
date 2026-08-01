//! The invariants that stop an account being charged twice.

use aex_payment_contracts::command::{CommandKind, EffectId, ProviderIdempotencyKey};
use aex_payment_contracts::result::{
    EffectState, HostedSession, PaymentFailure, PaymentFailureClass, PaymentResult, UnknownEvidence,
};
use aex_payment_contracts::{ProviderObjectRef, RedactedEmail};
use aex_wire::Uuid7;
use aex_wire::ids::{OrganizationId, PrefixedId};
use aex_wire::types::{HttpsUrl, StableCode, Timestamp};

fn organization() -> OrganizationId {
    OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("organization id")
}

fn effect() -> EffectId {
    EffectId(Uuid7::compose(
        1_785_501_296_789,
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    ))
}

#[test]
fn the_provider_idempotency_key_is_derived_and_never_invented() {
    let first =
        ProviderIdempotencyKey::derive(CommandKind::ChargeSavedMethod, organization(), effect());
    let second =
        ProviderIdempotencyKey::derive(CommandKind::ChargeSavedMethod, organization(), effect());
    assert_eq!(first, second, "a retry must present the same key");

    // Every input is part of the key, so no two effects can collide.
    assert_ne!(
        first,
        ProviderIdempotencyKey::derive(CommandKind::RefundCharge, organization(), effect())
    );
    let other_effect = EffectId(Uuid7::compose(
        1_785_501_296_790,
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    ));
    assert_ne!(
        first,
        ProviderIdempotencyKey::derive(
            CommandKind::ChargeSavedMethod,
            organization(),
            other_effect
        )
    );
    assert!(first.as_header_value().as_str().starts_with("aexk_"));
}

#[test]
fn a_provider_five_hundred_never_classifies_as_failed() {
    // This is the whole reason `OutcomeUnknown` exists. A 5xx, a timeout and a
    // transport loss all leave the charge indeterminate; recording any of them
    // as `Failed` invites a retry that charges twice.
    for class in [
        PaymentFailureClass::RateLimited,
        PaymentFailureClass::ProviderUnavailable,
        PaymentFailureClass::Permanent,
    ] {
        let failure = PaymentFailure {
            class,
            provider_code: Some(StableCode::parse("api_error").expect("code")),
            decline_code: None,
            retryable: true,
        };
        let result = PaymentResult::from_failure(effect(), failure);
        assert_eq!(
            result.state(),
            EffectState::OutcomeUnknown,
            "{class:?} must not settle as a determinate failure"
        );
    }
    for class in [
        PaymentFailureClass::CardDeclined,
        PaymentFailureClass::AuthenticationRequired,
        PaymentFailureClass::InvalidRequest,
    ] {
        let failure = PaymentFailure {
            class,
            provider_code: None,
            decline_code: None,
            retryable: false,
        };
        assert_eq!(
            PaymentResult::from_failure(effect(), failure).state(),
            EffectState::Failed,
            "{class:?} is determinate"
        );
    }
}

#[test]
fn the_effect_state_machine_fences_automatic_charging() {
    assert!(EffectState::Succeeded.permits_automatic_charge());
    assert!(EffectState::Failed.permits_automatic_charge());
    assert!(
        !EffectState::OutcomeUnknown.permits_automatic_charge(),
        "an indeterminate effect must fence the next automatic charge"
    );
    assert!(!EffectState::ManualReview.permits_automatic_charge());
    assert!(!EffectState::Prepared.permits_automatic_charge());

    assert!(EffectState::Prepared.can_transition_to(EffectState::OutcomeUnknown));
    assert!(EffectState::OutcomeUnknown.can_transition_to(EffectState::Succeeded));
    assert!(EffectState::OutcomeUnknown.can_transition_to(EffectState::ManualReview));
    assert!(
        !EffectState::Succeeded.can_transition_to(EffectState::Failed),
        "a settled effect is terminal"
    );
    assert!(!EffectState::Prepared.can_transition_to(EffectState::ManualReview));
}

#[test]
fn nothing_sensitive_renders_in_a_diagnostic() {
    let email = RedactedEmail::new("someone@example.test");
    let rendered = format!("{email:?}");
    assert!(!rendered.contains("someone"), "{rendered}");
    assert!(rendered.contains("example.test"), "{rendered}");

    let hosted = HostedSession {
        url: HttpsUrl::parse("https://pay.example.test/session/abc123").expect("url"),
        expires_at: Timestamp::from_unix_millis(1_785_501_296_789).expect("timestamp"),
    };
    let rendered = format!("{hosted:?}");
    assert!(
        !rendered.contains("abc123"),
        "a hosted payment URL leaked into Debug: {rendered}"
    );

    // A result never carries a raw provider message, only a stable code.
    let result = PaymentResult::Unknown {
        effect: effect(),
        evidence: UnknownEvidence::ServerError { status: 500 },
    };
    let json = serde_json::to_string(&result).expect("serialize");
    let decoded: PaymentResult = serde_json::from_str(&json).expect("decode");
    assert_eq!(decoded, result);
}

#[test]
fn a_succeeded_result_round_trips_through_the_cross_language_encoding() {
    let result = PaymentResult::Succeeded {
        effect: effect(),
        provider_ref: ProviderObjectRef("pi_test".to_owned()),
        provider_created_at: Timestamp::from_unix_millis(1_785_501_296_789).expect("timestamp"),
        hosted: None,
        charged: aex_wire::types::Cents::new(5000),
        tax: None,
    };
    let canonical = aex_wire::to_jcs_bytes(&result).expect("canonicalize");
    let decoded: PaymentResult =
        serde_json::from_slice(&canonical).expect("decode canonical bytes");
    assert_eq!(decoded, result);
    // Canonical bytes are stable, which is what the TypeScript edge compares to.
    assert_eq!(canonical, aex_wire::to_jcs_bytes(&decoded).expect("again"));
}

#[test]
fn the_crate_contains_no_floating_point_money() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("read src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read file");
            // Prose may name the thing it forbids; code may not contain it.
            let code: String = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join(" ");
            for token in ["f32", "f64"] {
                if code.contains(token) {
                    offenders.push(format!("{}: {token}", path.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "floating point found:\n{offenders:#?}"
    );
}
