//! G1(5): key templates round-trip, reject injected separators, and sort
//! identically to numeric and chronological order at the registered magnitudes.

use aex_observation_domain::keys::{
    self, BucketHour, KeyError, ObservationWakeKey, ScopeKey, SEQ_WIDTH,
};
use aex_observation_domain::signal::Signal;
use aex_wire::ids::PrefixedId;
use aex_wire::types::Timestamp;
use proptest::prelude::*;

fn session(suffix: &str) -> ScopeKey {
    ScopeKey::Session(
        PrefixedId::parse(&format!("ses_{suffix}")).expect("fixture session id parses"),
    )
}

fn workspace(suffix: &str) -> ScopeKey {
    ScopeKey::Workspace(
        PrefixedId::parse(&format!("ws_{suffix}")).expect("fixture workspace id parses"),
    )
}

const SUFFIX_A: &str = "01j0000000000000000000000a";
const SUFFIX_B: &str = "01j0000000000000000000000b";

#[test]
fn scope_keys_round_trip_through_their_rendering() {
    for scope in [session(SUFFIX_A), workspace(SUFFIX_B)] {
        let rendered = scope.to_key();
        let parsed = ScopeKey::parse(&rendered).expect("a rendered scope key parses");
        assert_eq!(parsed, scope, "round trip of {rendered}");
    }
}

#[test]
fn a_session_scope_renders_with_the_session_discriminator() {
    assert_eq!(session(SUFFIX_A).to_key(), format!("S#ses_{SUFFIX_A}"));
    assert_eq!(workspace(SUFFIX_B).to_key(), format!("W#ws_{SUFFIX_B}"));
}

#[test]
fn padding_is_exactly_twenty_characters_and_sorts_numerically() {
    assert_eq!(SEQ_WIDTH, 20);
    let boundaries: [u128; 6] = [
        0,
        1,
        u128::from(u64::MAX) - 1,
        u128::from(u64::MAX),
        u128::from(u64::MAX) + 1,
        99_999_999_999_999_999_999,
    ];
    let mut rendered: Vec<String> = boundaries.iter().copied().map(keys::pad_seq).collect();
    for text in &rendered {
        assert_eq!(text.len(), SEQ_WIDTH, "`{text}` is not {SEQ_WIDTH} wide");
    }
    let lexicographic = {
        let mut sorted = rendered.clone();
        sorted.sort();
        sorted
    };
    rendered.sort_by_key(|text| text.parse::<u128>().expect("padding stays numeric"));
    assert_eq!(rendered, lexicographic);
}

#[test]
fn padding_refuses_a_value_wider_than_the_field() {
    // 10^20 needs 21 digits and therefore cannot be padded into the fixed field.
    assert!(keys::try_pad_seq(100_000_000_000_000_000_000).is_none());
    assert!(keys::try_pad_seq(99_999_999_999_999_999_999).is_some());
}

#[test]
fn bucket_hours_are_thirteen_characters_and_sort_chronologically() {
    let early = BucketHour::from_timestamp(
        Timestamp::parse("2026-07-31T23:00:00.000Z").expect("fixture parses"),
    );
    let late = BucketHour::from_timestamp(
        Timestamp::parse("2026-08-01T00:00:00.000Z").expect("fixture parses"),
    );
    assert_eq!(early.as_str(), "2026-07-31T23");
    assert_eq!(late.as_str(), "2026-08-01T00");
    assert_eq!(early.as_str().len(), 13);
    assert!(early < late);
    assert_eq!(early.day(), "2026-07-31");
    assert_eq!(
        BucketHour::parse("2026-07-31T23").expect("a rendered bucket parses"),
        early
    );
}

#[test]
fn fixed_width_timestamps_sort_identically_to_chronological_order() {
    let mut instants = [
        Timestamp::parse("2026-08-01T00:00:00.001Z").expect("fixture parses"),
        Timestamp::parse("1970-01-01T00:00:00.000Z").expect("fixture parses"),
        Timestamp::parse("2026-07-31T23:59:59.999Z").expect("fixture parses"),
        Timestamp::parse("9999-12-31T23:59:59.999Z").expect("fixture parses"),
    ];
    let mut wire: Vec<String> = instants.iter().map(|value| value.to_wire()).collect();
    wire.sort();
    instants.sort();
    let chronological: Vec<String> = instants.iter().map(|value| value.to_wire()).collect();
    assert_eq!(wire, chronological);
}

#[test]
fn every_key_template_round_trips_its_components() {
    let scope = session(SUFFIX_A);
    let bucket = BucketHour::parse("2026-08-01T09").expect("fixture parses");
    for signal in Signal::AUTHORITY {
        for shard in [0_u8, 7, 63] {
            let pk = keys::observation_pk(&scope, *signal, bucket, shard);
            let parsed = keys::parse_observation_pk(&pk).expect("an observation pk parses");
            assert_eq!(
                parsed,
                ObservationWakeKey {
                    scope: scope.clone(),
                    signal: *signal,
                    abucket: bucket,
                    shard,
                }
            );
        }
    }
    assert_eq!(keys::observation_sk(42), keys::pad_seq(42));
    assert_eq!(keys::segment_pk(&scope, Signal::Logs), "SEG#S#ses_01j0000000000000000000000a#logs");
    assert_eq!(
        keys::time_segment_pk(&scope, Signal::Logs),
        "SEGT#S#ses_01j0000000000000000000000a#logs"
    );
    assert_eq!(
        keys::frontier_pk(&scope),
        "FRONT#S#ses_01j0000000000000000000000a"
    );
    assert_eq!(keys::frontier_sk(Signal::Metrics), "SIG#metrics");
    assert_eq!(keys::DELETION_SK, "DELETION");
}

#[test]
fn parse_observation_pk_rejects_every_other_item_family() {
    let scope = session(SUFFIX_A);
    let bucket = BucketHour::parse("2026-08-01T09").expect("fixture parses");
    let foreign = [
        keys::segment_pk(&scope, Signal::Logs),
        keys::time_segment_pk(&scope, Signal::Logs),
        keys::frontier_pk(&scope),
        "BATCH#ws_01j0000000000000000000000a#bch_01j0000000000000000000000a".to_owned(),
        "REJECT#ws_01j0000000000000000000000a#0".to_owned(),
        "SERIES#ws_01j0000000000000000000000a#00ff".to_owned(),
        "SERIESCT#ws_01j0000000000000000000000a".to_owned(),
        "QUOTA#ws_01j0000000000000000000000a".to_owned(),
        "GAP#S#ses_01j0000000000000000000000a".to_owned(),
        "SPOOL#ws_01j0000000000000000000000a#01".to_owned(),
        "EXPORT#ws_01j0000000000000000000000a#exp_01j0000000000000000000000a".to_owned(),
        "GATE#eu-west-1".to_owned(),
        "CTRL#spool.repair#00".to_owned(),
        "IDEM#ws_01j0000000000000000000000a#otlp.logs#ff".to_owned(),
        "OBS#".to_owned(),
        "OBSX#S#ses_01j0000000000000000000000a#logs#2026-08-01T09#00".to_owned(),
        String::new(),
    ];
    for pk in foreign {
        assert!(
            keys::parse_observation_pk(&pk).is_none(),
            "`{pk}` must not classify as an observation wake key"
        );
    }
    assert!(keys::parse_observation_pk(&keys::observation_pk(&scope, Signal::Logs, bucket, 0)).is_some());
}

#[test]
fn parse_observation_pk_rejects_the_events_signal() {
    // `events` live in `session-authority`; an `OBS#…#events#…` key can never be
    // written here, so classifying one would hide a corrupt item.
    let pk = "OBS#S#ses_01j0000000000000000000000a#events#2026-08-01T09#00";
    assert!(keys::parse_observation_pk(pk).is_none());
}

#[test]
fn a_component_may_not_carry_a_separator_or_a_sentinel() {
    for hostile in ["a#b", "a\0b", "a\u{ffff}b", "#", "\u{ffff}"] {
        assert!(
            matches!(keys::component(hostile), Err(KeyError::Separator { .. })),
            "`{}` must be refused as a key component",
            hostile.escape_debug()
        );
    }
    assert!(matches!(keys::component(""), Err(KeyError::Empty)));
    assert!(keys::component("plain-value.1").is_ok());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// No component the validator accepts can inject a separator, so every
    /// rendered key has exactly the field count its template declares.
    #[test]
    fn accepted_components_never_inject_a_separator(raw in ".{0,64}") {
        if let Ok(component) = keys::component(&raw) {
            prop_assert!(!component.as_str().contains('#'));
            prop_assert!(!component.as_str().contains('\0'));
            prop_assert!(!component.as_str().contains('\u{ffff}'));
            prop_assert!(!component.as_str().is_empty());
        } else {
            prop_assert!(
                raw.is_empty()
                    || raw.contains('#')
                    || raw.contains('\0')
                    || raw.contains('\u{ffff}')
                    || raw.len() > keys::COMPONENT_MAX_BYTES
            );
        }
    }

    /// Padding is order-preserving over the whole representable range.
    #[test]
    fn padding_preserves_order(a in 0_u128..=99_999_999_999_999_999_999,
                               b in 0_u128..=99_999_999_999_999_999_999) {
        let left = keys::pad_seq(a);
        let right = keys::pad_seq(b);
        prop_assert_eq!(a.cmp(&b), left.cmp(&right));
    }

    /// Every observation partition key the builder produces is classifiable.
    #[test]
    fn every_built_observation_pk_classifies(shard in 0_u8..64, signal_index in 0_usize..4) {
        let scope = session(SUFFIX_A);
        let signal = Signal::AUTHORITY[signal_index];
        let bucket = BucketHour::parse("2026-08-01T09").expect("fixture parses");
        let pk = keys::observation_pk(&scope, signal, bucket, shard);
        let parsed = keys::parse_observation_pk(&pk).expect("built keys classify");
        prop_assert_eq!(parsed.signal, signal);
        prop_assert_eq!(parsed.shard, shard);
        prop_assert_eq!(parsed.abucket, bucket);
    }
}
