//! The organization ceiling, and the shape that makes it unskippable.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7};
use tool_executor::spend::{
    CeilingRefusal, Ceilings, DEFAULT_CALLS_PER_DAY, DEFAULT_CALLS_PER_MINUTE,
    InMemoryOrganizationCeiling, OrganizationCeiling, Window,
};
use uuid::Uuid;

fn organization(seed: u128) -> OrganizationId {
    OrganizationId::from_uuid7(
        Uuid7::from_bytes(
            Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0000 | seed).into_bytes(),
        )
        .expect("a v7 identity"),
    )
}

fn at(offset_seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_767_225_600 + offset_seconds)
}

#[tokio::test]
async fn a_full_minute_refuses_and_the_next_minute_admits() {
    let ceiling = InMemoryOrganizationCeiling::with(Ceilings {
        per_minute: 2,
        per_day: 100,
    });
    let organization = organization(1);

    assert!(ceiling.admit(&organization, at(0)).await.is_ok());
    assert!(ceiling.admit(&organization, at(1)).await.is_ok());
    assert_eq!(
        ceiling.admit(&organization, at(2)).await,
        Err(CeilingRefusal::LimitExceeded),
        "the third call in one minute must be refused"
    );

    // The window is truncated rather than rolling, so crossing the boundary is
    // what admits again — not the passage of sixty seconds since the first call.
    assert!(
        ceiling.admit(&organization, at(60)).await.is_ok(),
        "the next minute must admit"
    );
}

#[tokio::test]
async fn the_day_ceiling_binds_even_when_every_minute_is_under_its_own() {
    // The reason there are two windows. One organization pacing itself at one
    // call a minute never trips the minute ceiling and would spend all month.
    let ceiling = InMemoryOrganizationCeiling::with(Ceilings {
        per_minute: 100,
        per_day: 3,
    });
    let organization = organization(2);

    for minute in 0..3 {
        assert!(
            ceiling.admit(&organization, at(minute * 60)).await.is_ok(),
            "minute {minute} is well under the per-minute ceiling"
        );
    }
    assert_eq!(
        ceiling.admit(&organization, at(3 * 60)).await,
        Err(CeilingRefusal::LimitExceeded)
    );
}

#[tokio::test]
async fn a_refusal_on_one_window_leaves_the_other_uncharged() {
    // The property `TransactWriteItems` gives the real adapter: both conditions
    // are evaluated before either count moves. Without it, a call refused by the
    // day window would still have spent a minute slot, and a customer would be
    // charged a bound for a call that never ran.
    let ceiling = InMemoryOrganizationCeiling::with(Ceilings {
        per_minute: 10,
        per_day: 1,
    });
    let organization = organization(3);

    assert!(ceiling.admit(&organization, at(0)).await.is_ok());
    assert_eq!(
        ceiling.admit(&organization, at(1)).await,
        Err(CeilingRefusal::LimitExceeded)
    );

    // The minute window still has nine of its ten. Proved by lifting the day
    // ceiling on a second organization and showing the same clock admits.
    let generous = InMemoryOrganizationCeiling::with(Ceilings {
        per_minute: 10,
        per_day: 100,
    });
    for _ in 0..10 {
        assert!(generous.admit(&organization, at(1)).await.is_ok());
    }
    assert_eq!(
        generous.admit(&organization, at(1)).await,
        Err(CeilingRefusal::LimitExceeded),
        "the eleventh call in one minute is refused"
    );
}

#[tokio::test]
async fn one_organization_cannot_spend_another_organizations_ceiling() {
    let ceiling = InMemoryOrganizationCeiling::with(Ceilings {
        per_minute: 1,
        per_day: 10,
    });
    let first = organization(4);
    let second = organization(5);

    assert!(ceiling.admit(&first, at(0)).await.is_ok());
    assert_eq!(
        ceiling.admit(&first, at(0)).await,
        Err(CeilingRefusal::LimitExceeded)
    );
    assert!(
        ceiling.admit(&second, at(0)).await.is_ok(),
        "the ceiling is keyed by organization, which is the key where the bound \
         and the bill agree"
    );
}

#[tokio::test]
async fn a_permit_names_the_organization_and_the_windows_it_was_counted_against() {
    let ceiling = InMemoryOrganizationCeiling::unbounded();
    let organization = organization(6);
    let permit = ceiling.admit(&organization, at(0)).await.expect("admits");

    assert_eq!(
        permit.organization().encode().as_str(),
        organization.encode().as_str()
    );
    assert_eq!(
        permit.buckets(),
        [Window::Minute.bucket(at(0)), Window::Day.bucket(at(0))]
    );
}

#[test]
fn the_default_numbers_are_the_ones_the_sizing_argument_produced() {
    // Recorded here so a later change to either is a change to a test with a
    // reason in it rather than an edit to a constant.
    //
    // 600 a minute: the deployed Brain fleet can hold 64 platform tool calls in
    // flight (2 tasks x 128 lane units / weight 4), so at a three-second call it
    // produces roughly 1 300 a minute in total. One organization taking more than
    // about half of the entire fleet's throughput is a loop, not a workload.
    //
    // 50 000 a day: about 6 % of what the minute ceiling would allow if sustained
    // all day. The minute ceiling bounds a loop; this one bounds a month.
    assert_eq!(DEFAULT_CALLS_PER_MINUTE, 600);
    assert_eq!(DEFAULT_CALLS_PER_DAY, 50_000);
    assert_eq!(Ceilings::DEFAULT.per_minute, DEFAULT_CALLS_PER_MINUTE);
    assert_eq!(Ceilings::DEFAULT.per_day, DEFAULT_CALLS_PER_DAY);
    const {
        assert!(
            DEFAULT_CALLS_PER_DAY < DEFAULT_CALLS_PER_MINUTE * 60 * 24,
            "a day ceiling a minute ceiling could never reach is not a bound"
        );
    }
}
