//! Bounded local load contracts for the paths every call runs through.
//!
//! The executor sits on a path the model is blocked on, and its caller holds an
//! activation, a lease and a network-lane permit for the whole wait. The things
//! that can be measured without a plane are the ones on that path before any
//! network call: argument decoding, envelope verification and window-key
//! derivation. The measurements that need a deployed plane — the added round
//! trip and the point at which admission starts queueing — live in
//! `tests/live/aex-live-tool-executor`, where they fail loudly instead of
//! pretending to have been taken.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_internal_contracts::tool_exec::{ArgumentsJcs, MAX_ARGUMENTS_JCS_BYTES};
use base64::Engine as _;
use tool_executor::spend::Window;

/// A fixed wall clock, built through a binding so the literal is a timestamp
/// rather than a duration a lint would rather see spelled in hours.
fn at(unix_seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(unix_seconds)
}

#[test]
fn an_oversized_argument_document_is_refused_before_it_is_decoded() {
    // Ten megabytes of base64 is 160 times the transport bound. The decoder must
    // refuse it from the encoded length, because the alternative is that an
    // unauthenticated body decides how much this process allocates.
    let hostile =
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(vec![b'{'; MAX_ARGUMENTS_JCS_BYTES * 160]);
    for _ in 0..100 {
        assert!(
            serde_json::from_value::<ArgumentsJcs>(serde_json::Value::String(hostile.clone()))
                .is_err()
        );
    }
}

#[test]
fn a_document_at_the_bound_decodes_and_is_bounded_at_the_bound() {
    let at_bound = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(vec![b'{'; MAX_ARGUMENTS_JCS_BYTES]);
    for _ in 0..100 {
        let decoded =
            serde_json::from_value::<ArgumentsJcs>(serde_json::Value::String(at_bound.clone()))
                .expect("the bound itself is admitted");
        assert_eq!(decoded.len(), MAX_ARGUMENTS_JCS_BYTES);
    }
}

#[test]
fn window_key_derivation_is_a_pure_function_and_stays_stable_under_load() {
    // Two windows are derived on every call, before the vendor request. They
    // must be pure: two processes that disagreed about which window a call falls
    // in would each enforce their own half of one ceiling.
    let at = at(1_767_225_659);
    let expected = [Window::Minute.bucket(at), Window::Day.bucket(at)];
    for _ in 0..100_000 {
        assert_eq!(
            [Window::Minute.bucket(at), Window::Day.bucket(at)],
            expected
        );
    }
}

#[test]
fn crossing_a_window_boundary_changes_exactly_one_key() {
    // A minute boundary must not move the day key, or a day ceiling would reset
    // sixty times an hour and stop bounding a month.
    let base = at(1_767_225_600);
    let next_minute = base + Duration::from_mins(1);
    assert_ne!(
        Window::Minute.bucket(base),
        Window::Minute.bucket(next_minute)
    );
    assert_eq!(Window::Day.bucket(base), Window::Day.bucket(next_minute));
}
