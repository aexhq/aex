//! G1(5): key templates round-trip, reject injected separators, and sort
//! identically to numeric and chronological order at the registered magnitudes.

use aex_observation_domain::keys::{
    self, BucketHour, KeyError, ObservationWakeKey, SEQ_WIDTH, ScopeKey,
};
use aex_observation_domain::signal::Signal;
use aex_wire::ids::PrefixedId;
use aex_wire::types::Timestamp;
use proptest::prelude::*;

fn session(suffix: &str) -> ScopeKey {
    session_in(SUFFIX_W, suffix)
}

fn session_in(workspace_suffix: &str, suffix: &str) -> ScopeKey {
    ScopeKey::Session {
        workspace: PrefixedId::parse(&format!("wsp_{workspace_suffix}"))
            .expect("fixture workspace id parses"),
        session: PrefixedId::parse(&format!("ses_{suffix}")).expect("fixture session id parses"),
    }
}

fn workspace(suffix: &str) -> ScopeKey {
    ScopeKey::Workspace(
        PrefixedId::parse(&format!("wsp_{suffix}")).expect("fixture workspace id parses"),
    )
}

const SUFFIX_A: &str = "0000000001e40r2081040g2081";
const SUFFIX_B: &str = "0000000002e81840g2081040g2";
/// The workspace every session fixture below belongs to.
const SUFFIX_W: &str = "0000000003ec1r60r30c1g60r3";

#[test]
fn scope_keys_round_trip_through_their_rendering() {
    for scope in [session(SUFFIX_A), workspace(SUFFIX_B)] {
        let rendered = scope.to_key();
        let parsed = ScopeKey::parse(&rendered).expect("a rendered scope key parses");
        assert_eq!(parsed, scope, "round trip of {rendered}");
    }
}

#[test]
fn a_session_scope_renders_the_workspace_before_the_session() {
    assert_eq!(
        session(SUFFIX_A).to_key(),
        format!("S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}")
    );
    assert_eq!(workspace(SUFFIX_B).to_key(), format!("W#wsp_{SUFFIX_B}"));
}

/// The property the OTLP ingest path's authorization now rests on.
///
/// `regional-otlp` takes the session from an untrusted `aex-session-id` header
/// and pairs it with the workspace its credential proved — deliberately without
/// a session→workspace lookup on the hot path. That is only sound if a session
/// identifier is powerless to move a row out of the workspace it was paired
/// with, so the property is asserted here, at the key, where it holds for every
/// caller rather than for one router.
#[test]
fn a_session_id_can_never_address_another_workspaces_partition() {
    // Two workspaces, and the *same* session identifier named inside each. This
    // is exactly what a caller supplying another tenant's session id produces.
    let mine = session_in(SUFFIX_W, SUFFIX_A);
    let theirs = session_in(SUFFIX_B, SUFFIX_A);
    assert_ne!(mine, theirs);
    assert_ne!(mine.to_key(), theirs.to_key());
    assert_eq!(mine.session(), theirs.session(), "the same session id");

    let bucket = BucketHour::parse("2026-08-01T09").expect("fixture parses");
    // Every partition family the scope keys, not just the observation rows.
    let mut families = vec![
        (keys::frontier_pk(&mine), keys::frontier_pk(&theirs)),
        (keys::gap_pk(&mine), keys::gap_pk(&theirs)),
        (
            keys::segment_pk(&mine, Signal::Logs),
            keys::segment_pk(&theirs, Signal::Logs),
        ),
        (
            keys::time_segment_pk(&mine, Signal::Logs),
            keys::time_segment_pk(&theirs, Signal::Logs),
        ),
    ];
    for signal in Signal::AUTHORITY {
        families.push((
            keys::observation_pk(&mine, *signal, bucket, 0),
            keys::observation_pk(&theirs, *signal, bucket, 0),
        ));
    }
    let owner = format!("wsp_{SUFFIX_W}");
    let stranger = format!("wsp_{SUFFIX_B}");
    for (ours, theirs) in families {
        assert_ne!(
            ours, theirs,
            "one session id addressed two workspaces' partitions"
        );
        assert!(
            ours.contains(&owner) && !ours.contains(&stranger),
            "`{ours}` must name its own workspace and no other"
        );
    }

    // And the wake classifier reports the owner, so a stream consumer never has
    // to resolve one.
    let pk = keys::observation_pk(&mine, Signal::Logs, bucket, 0);
    let classified = keys::parse_observation_pk(&pk).expect("a built key classifies");
    assert_eq!(classified.scope, mine);
    assert_eq!(
        classified.scope.workspace().to_string(),
        owner,
        "the partition key alone proves which tenant owns the row"
    );
}

#[test]
fn a_scope_component_with_a_missing_or_extra_field_is_refused() {
    for hostile in [
        // The pre-re-key session rendering, which named no workspace at all.
        &format!("S#ses_{SUFFIX_A}"),
        // Workspace and session transposed.
        &format!("S#ses_{SUFFIX_A}#wsp_{SUFFIX_W}"),
        // A fourth field smuggled past the two the template declares.
        &format!("S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}#extra"),
        &format!("W#wsp_{SUFFIX_W}#ses_{SUFFIX_A}"),
        &format!("X#wsp_{SUFFIX_W}"),
        &String::from("S"),
    ] {
        assert!(
            matches!(ScopeKey::parse(hostile), Err(KeyError::Malformed { .. })),
            "`{hostile}` must not parse as a scope"
        );
    }
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
                    scope,
                    signal: *signal,
                    abucket: bucket,
                    shard,
                }
            );
        }
    }
    assert_eq!(
        keys::segment_pk(&scope, Signal::Logs),
        format!("SEG#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}#logs")
    );
    assert_eq!(
        keys::time_segment_pk(&scope, Signal::Logs),
        format!("SEGT#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}#logs")
    );
    assert_eq!(
        keys::frontier_pk(&scope),
        format!("FRONT#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}")
    );
    assert_eq!(keys::frontier_sk(Signal::Metrics), "SIG#metrics");
    assert_eq!(keys::DELETION_SK, "DELETION");

    let batch =
        PrefixedId::parse("bch_0000000004g82840g2081040g2").expect("fixture batch id parses");
    let export =
        PrefixedId::parse("exp_0000000005gm2ga1850m2ga185").expect("fixture export id parses");
    assert_eq!(
        keys::scope_batch_pk(&scope),
        format!("BATCHS#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}")
    );
    assert_eq!(keys::scope_batch_sk(batch), batch.to_string());
    assert_eq!(keys::materialization_sk(7), "MAT#000007");
    assert_eq!(
        keys::scope_export_pk(&scope),
        format!("EXPORTS#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}")
    );
    assert_eq!(keys::scope_export_sk(export), export.to_string());
}

#[test]
fn parse_observation_pk_rejects_every_other_item_family() {
    let scope = session(SUFFIX_A);
    let bucket = BucketHour::parse("2026-08-01T09").expect("fixture parses");
    let foreign = [
        keys::segment_pk(&scope, Signal::Logs),
        keys::time_segment_pk(&scope, Signal::Logs),
        keys::frontier_pk(&scope),
        "BATCH#wsp_0000000001e40r2081040g2081#bch_0000000001e40r2081040g2081".to_owned(),
        "REJECT#wsp_0000000001e40r2081040g2081#0".to_owned(),
        "SERIES#wsp_0000000001e40r2081040g2081#00ff".to_owned(),
        "SERIESCT#wsp_0000000001e40r2081040g2081".to_owned(),
        "QUOTA#wsp_0000000001e40r2081040g2081".to_owned(),
        keys::gap_pk(&scope),
        "SPOOL#wsp_0000000001e40r2081040g2081#01".to_owned(),
        "EXPORT#wsp_0000000001e40r2081040g2081#exp_0000000001e40r2081040g2081".to_owned(),
        "GATE#eu-west-1".to_owned(),
        "CTRL#spool.repair#00".to_owned(),
        "IDEM#wsp_0000000001e40r2081040g2081#otlp.logs#ff".to_owned(),
        "OBS#".to_owned(),
        format!("OBSX#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}#logs#2026-08-01T09#00"),
        // A session key that names no workspace: the pre-re-key rendering can
        // never be resurrected by a stream record or a stored row.
        format!("OBS#S#ses_{SUFFIX_A}#logs#2026-08-01T09#00"),
        String::new(),
    ];
    for pk in foreign {
        assert!(
            keys::parse_observation_pk(&pk).is_none(),
            "`{pk}` must not classify as an observation wake key"
        );
    }
    assert!(
        keys::parse_observation_pk(&keys::observation_pk(&scope, Signal::Logs, bucket, 0))
            .is_some()
    );
}

#[test]
fn parse_observation_pk_rejects_the_events_signal() {
    // `events` live in `session-authority`; an `OBS#…#events#…` key can never be
    // written here, so classifying one would hide a corrupt item.
    let pk = format!("OBS#S#wsp_{SUFFIX_W}#ses_{SUFFIX_A}#events#2026-08-01T09#00");
    assert!(keys::parse_observation_pk(&pk).is_none());
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
        // The messages are spelled out rather than left to `stringify!`, because
        // a `\u{ffff}` escape inside a generated format string is itself a
        // format placeholder.
        if let Ok(component) = keys::component(&raw) {
            prop_assert!(!component.as_str().contains('#'), "accepted component carries the separator");
            prop_assert!(!component.as_str().contains('\0'), "accepted component carries NUL");
            prop_assert!(
                !component.as_str().contains('\u{ffff}'),
                "accepted component carries the sentinel"
            );
            prop_assert!(!component.as_str().is_empty(), "accepted component is empty");
        } else {
            let refusable = raw.is_empty()
                || raw.contains('#')
                || raw.contains('\0')
                || raw.contains('\u{ffff}')
                || raw.len() > keys::COMPONENT_MAX_BYTES;
            prop_assert!(refusable, "a benign component was refused");
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
