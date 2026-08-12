//! The invariants that make an internal envelope safe to deploy in stages.

use aex_internal_contracts::assertion::{
    AssertionAudience, AssertionError, AssertionRefusal, AssertionResponse, AudienceSet,
    CredentialDigest, IssuedAssertion, MAX_ASSERTION_TEXT_LEN, ResolveSessionForWorkspace,
    ResolveWorkspaceKey,
};
use aex_internal_contracts::control::{RegionalControlEnvelope, RegionalControlRequest};
use aex_internal_contracts::journal::JournalEntryKind;
use aex_internal_contracts::money::{MICROUSD_PER_CENT, Microusd, MicrousdDelta, MoneyError};
use aex_internal_contracts::outbox::{OutboxEvent, RunStatus, SessionRevision, UsageClosureId};
use aex_internal_contracts::usage::{AuthorityKind, FactAuthority, FactId, Meter, ServiceTime};
use aex_internal_contracts::{RunId, SchemaVersion};
use aex_wire::Uuid7;
use aex_wire::ids::{OrganizationId, PrefixedId, SessionId, UserId, WorkspaceId};
use aex_wire::types::{Cents, DecimalU128, Region, Timestamp};
use base64::Engine as _;

fn user() -> UserId {
    UserId::parse("usr_01kyw2qa4ne00r40r40m30e209").expect("user id")
}

fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("timestamp")
}

/// A stand-in envelope. This crate deliberately cannot build a real one: the
/// layout, the signature and the length belong to `aex-identity-domain`, and a
/// second constructor here would be a second definition of the artifact.
fn encoded_envelope() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 323])
}

#[test]
fn the_regional_control_payload_round_trips_at_the_contract_boundary() {
    let request = RegionalControlEnvelope {
        schema_version: SchemaVersion::V1,
        request_id: Uuid7::compose(1, [3; 10]),
        payload: RegionalControlRequest::ProvisionWorkspace {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            region: Region::EuWest1,
            fence: 7,
            intent_hash: "ab".repeat(32),
        },
    };
    let bytes = serde_json::to_vec(&request).expect("request encodes");
    let decoded: RegionalControlEnvelope<RegionalControlRequest> =
        serde_json::from_slice(&bytes).expect("request decodes");
    assert_eq!(decoded, request);
}

#[test]
fn an_account_pause_names_the_exact_epoch_that_every_regional_write_must_fence() {
    let request = RegionalControlEnvelope {
        schema_version: SchemaVersion::V1,
        request_id: Uuid7::compose(1, [4; 10]),
        payload: RegionalControlRequest::ApplyAccountPause {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            region: Region::EuWest1,
            account_epoch: 19,
        },
    };
    let document = serde_json::to_string(&request).expect("request encodes");
    assert!(document.contains(r#""request":"apply_account_pause""#));
    assert!(document.contains(r#""accountEpoch":19"#));
    assert_eq!(
        serde_json::from_str::<RegionalControlEnvelope<RegionalControlRequest>>(&document)
            .expect("request decodes"),
        request
    );
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
fn an_issued_assertion_has_exactly_one_textual_spelling() {
    let text = encoded_envelope();
    let issued = IssuedAssertion::new(text.clone()).expect("a canonical envelope");
    assert_eq!(issued.as_str(), text);

    // Padded and standard-alphabet spellings of the same bytes are refused, so
    // comparing two encoded envelopes is comparing their bytes.
    let padded = base64::engine::general_purpose::URL_SAFE.encode([7_u8; 323]);
    assert_eq!(IssuedAssertion::new(padded), Err(AssertionError::Encoding));
    assert_eq!(
        IssuedAssertion::new(base64::engine::general_purpose::STANDARD.encode([255_u8; 323])),
        Err(AssertionError::Encoding)
    );

    // A trailing character carrying non-zero unused bits is a second spelling
    // of the same bytes and is refused rather than silently normalised.
    let mut non_canonical = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0_u8; 4]);
    non_canonical.pop();
    non_canonical.push('B');
    assert_eq!(
        IssuedAssertion::new(non_canonical),
        Err(AssertionError::Encoding)
    );

    assert_eq!(IssuedAssertion::new(""), Err(AssertionError::Encoding));
    assert_eq!(
        IssuedAssertion::new("A".repeat(MAX_ASSERTION_TEXT_LEN + 1)),
        Err(AssertionError::Encoding)
    );
}

#[test]
fn an_issued_assertion_never_renders_its_envelope() {
    let issued = IssuedAssertion::new(encoded_envelope()).expect("a canonical envelope");
    let rendered = format!("{issued:?}");
    assert!(!rendered.contains(issued.as_str()), "{rendered}");
    let digest = CredentialDigest::new([9_u8; 32]);
    assert_eq!(
        format!("{digest:?}"),
        "CredentialDigest(<redacted:32 bytes>)"
    );
}

#[test]
fn neither_request_can_carry_a_credential() {
    // The stored verifier is a MAC over a digest precisely so the plaintext can
    // stay regional. A member named for the token itself must not exist, and
    // `deny_unknown_fields` makes one that is sent a decode failure.
    for document in [
        serde_json::to_string(&ResolveWorkspaceKey {
            schema_version: SchemaVersion::V1,
            key: aex_wire::ids::ApiKeyId::parse("key_01kyw2qa4ne00r40r40m30e209")
                .expect("api key id"),
            presented_digest: CredentialDigest::new([3_u8; 32]),
            region: Region::EuWest1,
            audience: AssertionAudience::RegionalSession,
        })
        .expect("a request encodes"),
        serde_json::to_string(&ResolveSessionForWorkspace {
            schema_version: SchemaVersion::V1,
            browser_session: SessionId::parse("ses_01kyw2qa4ne00r40r40m30e209")
                .expect("session id"),
            user: user(),
            workspace: aex_wire::ids::WorkspaceId::parse("wsp_01kyw2qa4ne00r40r40m30e209")
                .expect("workspace id"),
            presented_digest: CredentialDigest::new([3_u8; 32]),
            region: Region::EuWest1,
            audience: AssertionAudience::RegionalObservation,
        })
        .expect("a request encodes"),
    ] {
        assert!(!document.contains("credential\""), "{document}");
        assert!(!document.contains("token"), "{document}");
        assert!(document.contains("presentedDigest"), "{document}");
    }

    // An added member is refused rather than ignored: a caller that believed it
    // was sending a token must not be told the request succeeded.
    assert!(
        serde_json::from_str::<ResolveWorkspaceKey>(
            r#"{"schemaVersion":1,"key":"key_01kyw2qa4ne00r40r40m30e209","presentedDigest":"AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM","region":"eu-west-1","audience":"regional_session","credential":"aex_wk_euw1_x_y"}"#,
        )
        .is_err()
    );
}

#[test]
fn a_refusal_is_an_answer_and_carries_no_detail_a_caller_could_probe() {
    let refused = AssertionResponse::Refused {
        reason: AssertionRefusal::NotAuthorized,
    };
    let document = serde_json::to_string(&refused).expect("a refusal encodes");
    assert_eq!(
        document,
        r#"{"outcome":"refused","reason":"not_authorized"}"#
    );
    assert_eq!(
        serde_json::from_str::<AssertionResponse>(&document).expect("a refusal decodes"),
        refused
    );

    // Exactly two outcomes are distinguishable. "Unknown key", "wrong digest"
    // and "revoked" all collapse into one, so an unauthenticated caller cannot
    // learn which it was by asking.
    let unavailable = AssertionResponse::Refused {
        reason: AssertionRefusal::AccountStateUnavailable,
    };
    assert_ne!(refused, unavailable);

    let issued = AssertionResponse::Issued {
        assertion: IssuedAssertion::new(encoded_envelope()).expect("a canonical envelope"),
    };
    let document = serde_json::to_string(&issued).expect("an issue encodes");
    assert!(
        document.starts_with(r#"{"outcome":"issued","assertion":"#),
        "{document}"
    );
    assert_eq!(
        serde_json::from_str::<AssertionResponse>(&document).expect("an issue decodes"),
        issued
    );
}

#[test]
fn every_audience_names_exactly_one_deployable() {
    let mut seen: Vec<&str> = AssertionAudience::ALL
        .iter()
        .map(|audience| audience.deployable())
        .collect();
    assert_eq!(seen.len(), 5);
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 5, "two audiences named the same deployable");
    for audience in AssertionAudience::ALL {
        let document = serde_json::to_string(&audience).expect("an audience encodes");
        assert_eq!(
            serde_json::from_str::<AssertionAudience>(&document).expect("decodes"),
            audience
        );
        // The stored spelling on a projected key row and the serialized spelling
        // on the internal wire are one string, so a row written by the control
        // plane cannot name an audience a regional decoder spells differently.
        assert_eq!(document, format!("\"{}\"", audience.as_str()));
        assert_eq!(AssertionAudience::parse(audience.as_str()), Some(audience));
    }
    assert_eq!(AssertionAudience::parse("regional-session"), None);
    assert_eq!(AssertionAudience::parse(""), None);
}

#[test]
fn only_the_tool_executor_is_outside_what_a_customer_credential_may_name() {
    // The set a projected workspace-key row carries is the customer-presentable
    // one, and the single bit between it and the whole vocabulary is the tool
    // executor. Pinning the difference rather than the membership is what makes
    // a sixth audience a decision instead of an accident: whoever adds it has
    // to say which side it falls on, here.
    let outside: Vec<_> = AssertionAudience::ALL
        .into_iter()
        .filter(|audience| !AudienceSet::CUSTOMER_PRESENTABLE.contains(*audience))
        .collect();
    assert_eq!(outside, vec![AssertionAudience::ToolExec]);

    for audience in AssertionAudience::ALL {
        assert_eq!(
            AudienceSet::CUSTOMER_PRESENTABLE.contains(audience),
            audience.is_customer_presentable(),
            "{audience:?} is on two different sides of the same question"
        );
    }
    assert_ne!(AudienceSet::CUSTOMER_PRESENTABLE, AudienceSet::ALL);
}

#[test]
fn an_audience_set_admits_exactly_what_was_put_in_it() {
    assert!(AudienceSet::EMPTY.is_empty());
    assert!(!AudienceSet::ALL.is_empty());
    for audience in AssertionAudience::ALL {
        assert!(
            AudienceSet::ALL.contains(audience),
            "the full set must admit {audience:?}"
        );
        assert!(
            !AudienceSet::EMPTY.contains(audience),
            "the empty set admits nothing, which is what makes an unnamed row fail closed"
        );
        let one = AudienceSet::EMPTY.insert(audience);
        for other in AssertionAudience::ALL {
            assert_eq!(
                one.contains(other),
                other == audience,
                "a one-audience set admitted {other:?}"
            );
        }
        assert_eq!(one.insert(audience), one, "insertion is idempotent");
    }
    assert_eq!(
        AssertionAudience::ALL.into_iter().collect::<AudienceSet>(),
        AudienceSet::ALL
    );
    assert_eq!(
        AudienceSet::ALL.iter().collect::<Vec<_>>(),
        AssertionAudience::ALL.to_vec(),
        "iteration is wire order"
    );
    assert_eq!(
        AudienceSet::ALL.to_strings(),
        AssertionAudience::ALL
            .iter()
            .map(|audience| audience.as_str().to_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn declaration_order_is_bit_order() {
    // The bitset casts the discriminant, which is only the declaration index
    // while nobody has given a variant an explicit value.
    for (index, audience) in AssertionAudience::ALL.into_iter().enumerate() {
        let set = AudienceSet::EMPTY.insert(audience);
        let expected = AssertionAudience::ALL
            .get(index)
            .copied()
            .expect("the index is inside the vocabulary");
        assert!(set.contains(expected), "{audience:?} sits at bit {index}");
    }
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
fn the_terminal_outbox_event_decodes_on_this_side_of_the_boundary() {
    // `regional-stream` and the observation materializer both decode it, so it
    // is a contract envelope rather than a domain value. Declared in the session
    // domain it had no `Serialize` at all, which made "both decode it" a claim
    // nothing could satisfy.
    let event = OutboxEvent {
        schema_version: SchemaVersion::V1,
        session: SessionId::parse("ses_01kyw2qa4ne00r40r40m30e209").expect("session id"),
        run: RunId::parse("run_01kyw2qa4pew48j2gb1g6gw3rg").expect("run id"),
        status: RunStatus::Succeeded,
        session_revision: SessionRevision(7),
        usage_closure: UsageClosureId(Uuid7::compose(1_785_501_296_000, [3; 10])),
        at: timestamp(1_785_501_296_789),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    let decoded: OutboxEvent = serde_json::from_str(&json).expect("decode");
    assert_eq!(decoded, event);
    assert!(json.contains("\"sessionRevision\":\"7\""));

    // Strict on both halves: an unknown member and a missing version are typed
    // failures, not tolerated drift.
    let mut value: serde_json::Value = serde_json::from_str(&json).expect("value");
    value["surprise"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<OutboxEvent>(value).is_err());
}

#[test]
fn the_internal_outbox_run_status_vocabulary_is_stable() {
    // Public run resources no longer exist, but the private execution barrier
    // remains durable input to usage and observation materialization.
    let internal: Vec<String> = RunStatus::ALL
        .iter()
        .map(|status| serde_json::to_string(status).expect("serialize"))
        .collect();
    assert_eq!(
        internal,
        [
            "\"queued\"",
            "\"running\"",
            "\"succeeded\"",
            "\"failed\"",
            "\"timed_out\"",
            "\"cancelled\"",
            "\"interrupted\"",
        ]
    );
    assert_eq!(
        RunStatus::ALL
            .iter()
            .filter(|status| status.is_terminal())
            .count(),
        5
    );
}
