//! Slice S-1.3 — the bounded journal decoder over arbitrary bytes.
//!
//! The decoder must never panic, never allocate unboundedly, and never accept a
//! non-canonical encoding. The third is the one that matters most: accepting a
//! non-canonical body would let a second encoder exist anywhere in the system without
//! anything failing, which is exactly what the one-canonicalizer rule forbids.

use aex_brain_domain::canonical::{CanonicalizeError, canonicalize, canonicalize_value};
use aex_brain_domain::journal::{INLINE_BODY_BYTES, JournalDecodeError, decode};
use aex_brain_test_support::histories::all;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 2_048, ..ProptestConfig::default() })]

    /// Arbitrary bytes decode to a typed error or a record; never a panic.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4_096)) {
        let _ = decode(&bytes);
    }

    /// Arbitrary ASCII, which is far likelier to reach the parser than random bytes.
    #[test]
    fn arbitrary_text_never_panics(text in "\\PC{0,512}") {
        let _ = decode(text.as_bytes());
    }

    /// A body over the inline boundary is refused by length before anything is parsed, so
    /// an oversized body costs a comparison rather than an allocation.
    #[test]
    fn an_oversized_body_is_refused_before_parsing(extra in 1_usize..4_096) {
        let bytes = vec![b'{'; INLINE_BODY_BYTES + extra];
        let error = decode(&bytes).expect_err("an oversized body is refused");
        prop_assert!(
            matches!(error, JournalDecodeError::TooLarge { len, limit }
                if len == INLINE_BODY_BYTES + extra && limit == INLINE_BODY_BYTES),
            "{error:?}"
        );
    }

    /// Canonicalization is idempotent, so a decoded record re-encodes to the bytes it came
    /// from. If it did not, the decoder's canonical check would reject valid data.
    #[test]
    fn canonicalization_reaches_a_fixed_point(
        depth in 0_usize..6,
        width in 0_usize..6,
    ) {
        let mut value = serde_json::json!({ "leaf": 1, "text": "x" });
        for level in 0..depth {
            let mut object = serde_json::Map::new();
            for index in 0..width.max(1) {
                object.insert(format!("k{level}_{index}"), value.clone());
            }
            value = serde_json::Value::Object(object);
        }
        let once = match canonicalize(&value) {
            Ok(bytes) => bytes,
            Err(CanonicalizeError::TooLarge | CanonicalizeError::TooDeep { .. }) => return Ok(()),
            Err(other) => return Err(TestCaseError::fail(format!("{other:?}"))),
        };
        let reparsed: serde_json::Value =
            serde_json::from_slice(&once).expect("canonical output reparses");
        prop_assert_eq!(once, canonicalize(&reparsed).expect("the second pass succeeds"));
    }
}

/// Every record in the golden corpus round-trips through the decoder unchanged.
#[test]
fn every_golden_record_round_trips() {
    for case in all() {
        for entry in &case.history {
            let bytes = entry
                .record
                .canonical_bytes()
                .expect("a fixture record canonicalizes");
            if bytes.len() > INLINE_BODY_BYTES {
                continue;
            }
            let decoded = decode(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{}: {} did not round trip: {error}",
                    case.name,
                    entry.record.kind_name()
                )
            });
            assert_eq!(decoded, entry.record, "{}", case.name);
        }
    }
}

/// A body that parses but was not written canonically is refused.
///
/// This is what stops a second encoder from existing unnoticed anywhere in the system.
#[test]
fn a_parseable_but_non_canonical_body_is_refused() {
    let record = &all()
        .into_iter()
        .find(|case| !case.history.is_empty())
        .expect("the corpus is non-empty")
        .history[0]
        .record
        .clone();
    let canonical = record.canonical_bytes().expect("the record canonicalizes");

    // Re-serialize through `serde_json`'s pretty printer: the same document, a different
    // encoding.
    let value: serde_json::Value =
        serde_json::from_slice(&canonical).expect("the canonical bytes parse");
    let pretty = serde_json::to_vec_pretty(&value).expect("the value re-serializes");
    assert_ne!(pretty, canonical, "the fixture must actually differ");

    let error = decode(&pretty).expect_err("a non-canonical encoding is refused");
    assert!(
        matches!(error, JournalDecodeError::NonCanonical),
        "{error:?}"
    );
}

/// An unknown record kind is a decode error rather than a forward-compatible passthrough.
#[test]
fn an_unknown_kind_is_a_decode_error() {
    let bytes = canonicalize_value(&serde_json::json!({ "record": "telepathy" }))
        .expect("the probe canonicalizes");
    let error = decode(&bytes).expect_err("an unknown kind is refused");
    assert!(
        matches!(error, JournalDecodeError::Malformed { .. }),
        "{error:?}"
    );
}

/// Prelaunch is a clean cut: a journal written before provider-credential
/// authority existed cannot be reopened and dispatched under a mutable default.
#[test]
fn agent_started_without_an_immutable_credential_pin_is_refused() {
    let started = all()
        .into_iter()
        .flat_map(|case| case.history)
        .find(|entry| entry.record.kind_name() == "agent_started")
        .expect("the corpus carries an agent start");
    let mut value = serde_json::to_value(started.record).expect("the record serializes");
    value
        .get_mut("config")
        .and_then(serde_json::Value::as_object_mut)
        .expect("agent_started has a config")
        .remove("credential")
        .expect("the current config carries a credential pin");
    let bytes = canonicalize_value(&value).expect("the hostile record canonicalizes");
    assert!(
        matches!(decode(&bytes), Err(JournalDecodeError::Malformed { .. })),
        "a missing pin must never fall back to mutable credential state"
    );
}

#[test]
fn zero_credential_revision_or_generation_is_refused() {
    let started = all()
        .into_iter()
        .flat_map(|case| case.history)
        .find(|entry| entry.record.kind_name() == "agent_started")
        .expect("the corpus carries an agent start");
    let baseline = serde_json::to_value(started.record).expect("the record serializes");
    for field in ["revision", "generation"] {
        let mut value = baseline.clone();
        value["config"]["credential"][field] = serde_json::json!(0);
        let bytes = canonicalize_value(&value).expect("the hostile record canonicalizes");
        assert!(
            matches!(decode(&bytes), Err(JournalDecodeError::Malformed { .. })),
            "zero {field} must never enter immutable session authority"
        );
    }
}

/// A float anywhere in a canonical body is rejected, not silently reformatted.
///
/// Money is integer micro-USD and every Brain quantity is an integer, so a float is an
/// upstream defect. Reformatting it would hide the defect and change a hash.
#[test]
fn a_float_is_rejected_rather_than_reformatted() {
    let error = canonicalize(&serde_json::json!({ "nested": { "cost": 0.1 } }))
        .expect_err("a float is refused");
    assert!(
        matches!(error, CanonicalizeError::NonIntegerNumber { ref path, .. } if path == "/nested/cost"),
        "{error:?}"
    );
}
