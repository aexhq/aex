//! The invariants that make an internal envelope safe to deploy in stages.

use aex_internal_contracts::assertion::{
    AssertionAudience, AssertionError, AuthorizationAssertion, MAX_LIFETIME_MS, signing_input,
};
use aex_internal_contracts::journal::JournalEntryKind;
use aex_internal_contracts::money::{MICROUSD_PER_CENT, Microusd, MicrousdDelta, MoneyError};
use aex_internal_contracts::usage::{AuthorityKind, FactAuthority, FactId, Meter, ServiceTime};
use aex_internal_contracts::{Epoch, SchemaVersion};
use aex_wire::idempotency::{PrincipalKind, PrincipalScope};
use aex_wire::ids::{OrganizationId, PrefixedId, UserId};
use aex_wire::scopes::{ScopeId, ScopeSet};
use aex_wire::types::{Cents, DecimalU128, Region, Timestamp};

fn organization() -> OrganizationId {
    OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("organization id")
}

fn user() -> UserId {
    UserId::parse("usr_01kyw2qa4ne00r40r40m30e209").expect("user id")
}

fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("timestamp")
}

fn assertion(lifetime_ms: i64) -> AuthorizationAssertion {
    AuthorizationAssertion {
        schema_version: SchemaVersion::V1,
        principal: PrincipalScope::Account {
            user: user(),
            organization: Some(organization()),
        },
        principal_kind: PrincipalKind::Account,
        workspace: None,
        organization: organization(),
        region: Region::EuWest1,
        scopes: ScopeSet::new([ScopeId::AccountRead, ScopeId::SessionsRead]),
        key_epoch: Epoch(1),
        account_epoch: Epoch(2),
        revocation_epoch: Epoch(3),
        issued_at: timestamp(1_785_501_296_000),
        expires_at: timestamp(1_785_501_296_000 + lifetime_ms),
        audience: AssertionAudience::RegionalSession,
    }
}

#[test]
fn one_cent_is_exactly_ten_thousand_microusd() {
    assert_eq!(MICROUSD_PER_CENT, 10_000);
    let amount = Microusd::from_cents(Cents::new(1234)).expect("convert");
    assert_eq!(amount.get(), 12_340_000);
    assert_eq!(amount.to_cents_exact().expect("back"), Cents::new(1234));
    // A fraction of a cent may not silently round; rounding is a transfer.
    assert_eq!(
        Microusd::new(12_340_001).to_cents_exact(),
        Err(MoneyError::Overflow)
    );
    assert_eq!(
        Microusd::from_cents(Cents::new(u64::MAX)),
        Err(MoneyError::Overflow)
    );
}

#[test]
fn a_balance_can_never_be_pushed_below_zero_silently() {
    let balance = Microusd::new(10_000);
    let debit = MicrousdDelta::debit(Microusd::new(15_000)).expect("delta");
    assert_eq!(balance.checked_add(debit), Err(MoneyError::Negative));
    let credit = MicrousdDelta::credit(Microusd::new(5_000)).expect("delta");
    assert_eq!(
        balance.checked_add(credit).expect("credit"),
        Microusd::new(15_000)
    );
    assert_eq!(
        Microusd::new(u64::MAX).checked_add(credit),
        Err(MoneyError::Overflow)
    );
}

#[test]
fn an_assertion_may_not_outlive_thirty_seconds() {
    assert_eq!(MAX_LIFETIME_MS, 30_000);
    assert!(assertion(30_000).validate().is_ok());
    assert_eq!(
        assertion(30_001).validate(),
        Err(AssertionError::LifetimeTooLong)
    );
    assert_eq!(
        assertion(0).validate(),
        Err(AssertionError::NotForwardInTime)
    );
}

#[test]
fn the_signing_input_is_domain_separated_and_canonical() {
    let bytes = signing_input(&assertion(30_000));
    assert!(
        bytes.starts_with(b"aex:authorization-assertion:v1\x1f"),
        "the signing input must be domain separated"
    );
    // Same value, same bytes: a signature is only meaningful if this holds.
    assert_eq!(bytes, signing_input(&assertion(30_000)));
    // A different value must not produce the same bytes.
    assert_ne!(bytes, signing_input(&assertion(29_000)));
}

#[test]
fn a_fact_identity_is_derived_and_therefore_repeatable() {
    let authority = FactAuthority {
        kind: AuthorityKind::Compute,
        authority_id: "gen_01kyw2qa4ne00r40r40m30e209".into(),
        segment_ordinal: DecimalU128::new(3),
    };
    let first = FactId::derive(Region::EuWest1, Meter::ComputeMillicpuMs, &authority);
    let second = FactId::derive(Region::EuWest1, Meter::ComputeMillicpuMs, &authority);
    assert_eq!(
        first, second,
        "a producer retry must mint the same identity"
    );
    assert_ne!(
        first,
        FactId::derive(Region::UsEast1, Meter::ComputeMillicpuMs, &authority),
        "region is part of the identity"
    );
    assert_ne!(
        first,
        FactId::derive(Region::EuWest1, Meter::MemoryByteMs, &authority),
        "meter is part of the identity"
    );
    let json = serde_json::to_string(&first).expect("serialize");
    let decoded: FactId = serde_json::from_str(&json).expect("decode");
    assert_eq!(decoded, first);
}

#[test]
fn exactly_four_meters_are_priced() {
    assert_eq!(Meter::ALL.len(), 4);
    let names: Vec<&str> = Meter::ALL.iter().map(|meter| meter.as_str()).collect();
    assert_eq!(
        names,
        [
            "compute.millicpu_ms.v1",
            "memory.byte_ms.v1",
            "storage.byte_min.v1",
            "data_transfer.egress_byte.v1",
        ]
    );
    // Model tokens are zero-dollar observability and must not be priceable here.
    assert!(!names.iter().any(|name| name.contains("token")));
}

#[test]
fn the_journal_kind_vocabulary_is_closed_and_split_by_writer() {
    assert_eq!(JournalEntryKind::ALL.len(), 22);
    let authority_only: Vec<JournalEntryKind> = JournalEntryKind::ALL
        .into_iter()
        .filter(|kind| kind.is_authority_only())
        .collect();
    assert!(
        authority_only.contains(&JournalEntryKind::SessionDeleted),
        "a deletion must not be forgeable by an execution writer"
    );
    assert!(!JournalEntryKind::AssistantMessage.is_authority_only());

    // Round-trips exactly, and an unknown kind is a decode error rather than a
    // value that would slip through the authority fold.
    for kind in JournalEntryKind::ALL {
        let json = serde_json::to_string(&kind).expect("serialize");
        let decoded: JournalEntryKind = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, kind);
    }
    assert!(serde_json::from_str::<JournalEntryKind>("\"something_new\"").is_err());
}

#[test]
fn every_envelope_rejects_an_unknown_member() {
    let json = serde_json::json!({
        "kind": "compute",
        "authorityId": "gen_01kyw2qa4ne00r40r40m30e209",
        "segmentOrdinal": "3",
        "surprise": true,
    });
    assert!(
        serde_json::from_value::<FactAuthority>(json).is_err(),
        "an unknown member must be rejected, not ignored"
    );
    // The tagged unions still discriminate strictly on their tag.
    assert!(
        serde_json::from_value::<ServiceTime>(serde_json::json!({
            "kind": "eventually",
            "at": "2026-07-31T12:34:56.789Z",
        }))
        .is_err(),
        "an unknown discriminator must be rejected"
    );
}

#[test]
fn the_crate_contains_no_floating_point_money() {
    // U-RATING forbids floating-point money inside authorities. The cheapest way
    // to keep that true is to make a float unwritable in the crate at all.
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
