//! G2(15) — the OTLP decoder is an untrusted parser and a declared fuzz target.
//!
//! The property is the fuzz property: **no panic, on any input, on any route**.
//! It runs in the default lane rather than only in a separate fuzz job, because
//! a fuzz target nothing gates on is not a gate.

use aex_otlp_admission::decode::{ContentCoding, DecodeRequest, OtlpEncoding, decode};
use aex_otlp_admission::error::OtlpSignal;
use aex_otlp_admission::limits::OtlpLimits;
use aex_otlp_admission::memory::{MemoryBudget, reservation_for};
use proptest::prelude::*;

fn attempt(signal: OtlpSignal, json: bool, gzip: bool, body: &[u8]) -> bool {
    let limits = OtlpLimits {
        encoded_max: 16 * 1024,
        decoded_max: 128 * 1024,
        ..OtlpLimits::REGISTERED
    };
    let budget = MemoryBudget::new(1 << 20);
    let coding = if gzip {
        ContentCoding::Gzip
    } else {
        ContentCoding::Identity
    };
    let reservation = reservation_for(
        body.len(),
        coding.expansion_estimate(limits.max_ratio),
        limits.decoded_max,
    );
    let Ok(lease) = budget.try_reserve(reservation) else {
        return true;
    };
    let outcome = decode(&DecodeRequest {
        signal,
        encoding: if json {
            OtlpEncoding::Json
        } else {
            OtlpEncoding::Protobuf
        },
        coding,
        body,
        limits: &limits,
        lease: &lease,
    });
    drop(lease);
    // Whatever happened, the budget must be released and the record count bounded.
    assert_eq!(budget.reserved(), 0, "a decode leaked a memory permit");
    match outcome {
        Ok(batch) => batch.record_count() <= limits.max_records,
        // A typed error is a correct outcome; only a panic fails the case.
        Err(_) => true,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(768))]

    /// Arbitrary bytes on any route, encoding and coding: no panic, ever.
    #[test]
    fn arbitrary_bytes_never_panic_and_never_leak_a_permit(
        raw in proptest::collection::vec(any::<u8>(), 0..2048),
        signal_index in 0_usize..3,
        json in any::<bool>(),
        gzip in any::<bool>(),
    ) {
        prop_assert!(attempt(OtlpSignal::ALL[signal_index], json, gzip, &raw));
    }

    /// Arbitrary text on the JSON route: no panic, ever.
    #[test]
    fn arbitrary_text_never_panics_on_the_json_route(text in ".{0,512}") {
        prop_assert!(attempt(OtlpSignal::Logs, true, false, text.as_bytes()));
    }

    /// A gzip frame built from arbitrary bytes: no panic, ever.
    #[test]
    fn arbitrary_gzip_frames_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..1024)) {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&raw).expect("in-memory write");
        let framed = encoder.finish().expect("in-memory finish");
        prop_assert!(attempt(OtlpSignal::Metrics, false, true, &framed));
    }
}
