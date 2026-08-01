//! Failure-path cases for `regional-work`.

mod support;

use aex_session_dynamodb::error::StoreError;
use aex_work_dynamodb::WorkClaim;
use aex_work_dynamodb::claim::{claim, complete, poison, renew};
use aex_work_dynamodb::codec::{Payload, PayloadError, encode_work};

use support::{TABLE, later, now, record};

fn hold() -> WorkClaim {
    WorkClaim {
        work_id: record().work_id,
        fence: 3,
        owner: "worker-1".to_owned(),
        attempt: 1,
        lease_expires_at: later(30_000),
    }
}

#[test]
fn a_lost_fence_has_no_expression_that_could_be_retried_into_success() {
    // Every post-claim expression conditions on the exact fence and owner, so a
    // worker whose lease was stolen cannot re-issue the same write and win.
    for builder in [
        renew(TABLE, &hold(), now(), later(60_000)).expect("builds"),
        complete(TABLE, &hold(), now()).expect("builds"),
        poison(TABLE, &hold(), "boom", now()).expect("builds"),
    ] {
        let built = builder.build().expect("a complete update");
        let condition = built.condition_expression().expect("conditional");
        assert!(condition.contains("fence = :fence"), "{condition}");
        assert!(condition.contains("claimOwner = :owner"), "{condition}");
    }
}

#[test]
fn a_claim_beyond_the_attempt_budget_can_never_be_expressed() {
    let built = claim(TABLE, &record().work_id, "worker-1", now(), later(1))
        .expect("builds")
        .build()
        .expect("a complete update");
    assert!(
        built
            .condition_expression()
            .expect("conditional")
            .contains("attempt < maxAttempts"),
        "the attempt budget must be part of the claim condition, not a later check"
    );
}

#[test]
fn an_unknown_kind_fails_before_a_row_is_ever_built() {
    let mut invented = record();
    invented.kind = "agent.teleport".to_owned();
    let error = encode_work(&invented).expect_err("an unknown kind");
    assert!(error.to_string().contains("agent.teleport"), "{error}");
}

#[test]
fn a_payload_carrying_an_undeclared_member_is_refused_with_the_member_named() {
    let error = Payload::new()
        .set("apiKey", "sk-live-something")
        .check("agent.wake")
        .expect_err("undeclared");
    assert_eq!(
        error,
        PayloadError::UndeclaredMember {
            kind: "agent.wake".to_owned(),
            member: "apiKey".to_owned(),
        }
    );
}

#[test]
fn a_key_component_that_could_forge_a_key_stops_every_expression_builder() {
    for outcome in [
        claim(TABLE, "wrk#evil", "worker-1", now(), later(1)).err(),
        renew(
            TABLE,
            &WorkClaim {
                work_id: "wrk#evil".to_owned(),
                ..hold()
            },
            now(),
            later(1),
        )
        .err(),
    ] {
        let error = outcome.expect("a key error");
        assert!(matches!(error, StoreError::Key(_)), "{error}");
    }
}

#[test]
fn a_priority_band_outside_the_declared_table_fails_the_encode() {
    let mut invented = record();
    invented.priority = 9;
    assert!(
        encode_work(&invented).is_err(),
        "an unknown priority band must not silently become band zero"
    );
}
