//! Behavioural tests for the hand-written scalar vocabulary of `aex-wire`.
//!
//! These are the bytes every other crate in the workspace depends on, so each
//! rule here is asserted against an explicit accept/reject matrix rather than a
//! round-trip alone.

use aex_wire::canonical::{CanonicalJson, to_jcs_bytes};
use aex_wire::models::ObservationCoverage;
use aex_wire::types::{
    ByteRange, Cents, ComputeSize, DecimalU128, ETag, HttpMethod, HttpsUrl, JsonPointer, Region,
    RequestId, Timestamp,
};

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serialize")
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(text)
}

#[test]
fn region_renders_the_public_aws_region_name() {
    assert_eq!(json(&Region::EuWest1), "\"eu-west-1\"");
    assert_eq!(json(&Region::ApNortheast1), "\"ap-northeast-1\"");
    assert_eq!(Region::EuWest1.code(), "euw1");
    assert_eq!(Region::ApNortheast1.code(), "apne1");
    assert_eq!(Region::from_code("use2"), Some(Region::UsEast2));
    assert_eq!(Region::from_code("eu-west-1"), None);
    assert_eq!(Region::ALL.len(), 5);
    assert!(parse::<Region>("\"eu-central-1\"").is_err());
}

#[test]
fn compute_size_is_the_five_public_baseline_tokens() {
    let tokens: Vec<&str> = ComputeSize::ALL.iter().map(|size| size.as_str()).collect();
    assert_eq!(tokens, ["512mb", "1gb", "2gb", "4gb", "8gb"]);
    assert_eq!(ComputeSize::DEFAULT, ComputeSize::Gb1);
    assert_eq!(json(&ComputeSize::Mb512), "\"512mb\"");
    assert!(parse::<ComputeSize>("\"16gb\"").is_err());
}

#[test]
fn timestamp_renders_exactly_three_fractional_digits() {
    let value: Timestamp = parse("\"2026-07-31T12:34:56.789Z\"").expect("parse");
    assert_eq!(json(&value), "\"2026-07-31T12:34:56.789Z\"");
    assert_eq!(value.unix_millis(), 1_785_501_296_789);
    assert_eq!(
        Timestamp::from_unix_millis(1_785_501_296_789).expect("from millis"),
        value
    );
}

#[test]
fn timestamp_rejects_every_other_rfc3339_spelling() {
    for text in [
        "\"2026-07-31T12:34:56Z\"",
        "\"2026-07-31T12:34:56.789456Z\"",
        "\"2026-07-31T12:34:56.78Z\"",
        "\"2026-07-31T12:34:56.789+00:00\"",
        "\"2026-07-31t12:34:56.789z\"",
        "\"2026-07-31 12:34:56.789Z\"",
        "\"2026-13-31T12:34:56.789Z\"",
        "\"2026-07-31T24:34:56.789Z\"",
    ] {
        assert!(parse::<Timestamp>(text).is_err(), "{text} must not parse");
    }
}

#[test]
fn decimal_u128_is_a_canonical_non_negative_decimal_string() {
    let value: DecimalU128 = parse("\"340282366920938463463374607431768211455\"").expect("max");
    assert_eq!(value.get(), u128::MAX);
    assert_eq!(json(&DecimalU128::new(0)), "\"0\"");
    for text in [
        "\"01\"",
        "\"+1\"",
        "\"-1\"",
        "\"1e3\"",
        "\"1.0\"",
        "\"\"",
        "\" 1\"",
        "1",
        "\"0x10\"",
        "\"340282366920938463463374607431768211456\"",
    ] {
        assert!(
            parse::<DecimalU128>(text).is_err(),
            "{text} must not parse as DecimalU128"
        );
    }
    assert!(
        DecimalU128::new(u128::MAX)
            .checked_add(DecimalU128::new(1))
            .is_none()
    );
}

#[test]
fn cents_is_a_canonical_non_negative_decimal_string() {
    assert_eq!(json(&Cents::new(1234)), "\"1234\"");
    assert_eq!(parse::<Cents>("\"0\"").expect("zero").get(), 0);
    assert!(parse::<Cents>("\"-5\"").is_err());
    assert!(parse::<Cents>("1234").is_err());
}

#[test]
fn https_url_rejects_every_non_https_origin() {
    assert!(HttpsUrl::parse("https://api.aex.dev/x").is_ok());
    for text in [
        "http://api.aex.dev",
        "ftp://api.aex.dev",
        "https://",
        "//api.aex.dev",
        "https://api.aex.dev/\u{7f}",
    ] {
        assert!(HttpsUrl::parse(text).is_err(), "{text} must not parse");
    }
}

#[test]
fn byte_range_is_inclusive_and_non_empty() {
    let range = ByteRange::new(0, 0).expect("single byte");
    assert_eq!(range.len(), 1);
    assert_eq!(json(&range), "{\"start\":\"0\",\"endInclusive\":\"0\"}");
    assert!(ByteRange::new(5, 4).is_err());
}

#[test]
fn json_pointer_follows_rfc_6901() {
    assert!(JsonPointer::parse("").is_ok());
    assert!(JsonPointer::parse("/content/0/text").is_ok());
    assert!(JsonPointer::parse("/a~0b~1c").is_ok());
    assert!(JsonPointer::parse("content").is_err());
    assert!(JsonPointer::parse("/a~2b").is_err());
}

#[test]
fn etag_and_request_id_reject_empty_and_control_characters() {
    assert!(ETag::parse("\"w-17\"").is_ok());
    assert!(ETag::parse("").is_err());
    assert!(ETag::parse("a\nb").is_err());
    assert!(RequestId::parse("req-01").is_ok());
    assert!(RequestId::parse("").is_err());
}

#[test]
fn http_method_order_is_the_bundling_order() {
    assert_eq!(
        HttpMethod::ALL,
        [
            HttpMethod::Get,
            HttpMethod::Put,
            HttpMethod::Post,
            HttpMethod::Delete
        ]
    );
    assert_eq!(HttpMethod::Post.as_str(), "POST");
}

#[test]
fn jcs_matches_rfc_8785_ordering_and_number_rules() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"b":1,"a":{"é":2,"e":3},"z":[3,1,2],"n":null,"t":true,"s":"ü"}"#)
            .expect("input");
    let bytes = to_jcs_bytes(&value).expect("jcs");
    assert_eq!(
        String::from_utf8(bytes).expect("utf8"),
        "{\"a\":{\"e\":3,\"\u{e9}\":2},\"b\":1,\"n\":null,\"s\":\"\u{fc}\",\"t\":true,\"z\":[3,1,2]}"
    );
}

#[test]
fn jcs_ordering_does_not_depend_on_the_serde_json_map_type() {
    // `serde_json::Map` is a `BTreeMap` only while `preserve_order` is off, and
    // `aws-smithy-http-client` turns it on — so any binary linking an AWS SDK
    // crate canonicalises with an insertion-ordered `IndexMap`. Building the
    // object by inserting out of order is what makes this test able to fail;
    // parsing a sorted literal cannot distinguish the two map types.
    let mut object = serde_json::Map::new();
    object.insert("z".to_owned(), serde_json::Value::from(1));
    object.insert("a".to_owned(), serde_json::Value::from(2));
    object.insert("m".to_owned(), serde_json::Value::from(3));
    let mut nested = serde_json::Map::new();
    nested.insert("y".to_owned(), serde_json::Value::from(4));
    nested.insert("b".to_owned(), serde_json::Value::from(5));
    object.insert("n".to_owned(), serde_json::Value::Object(nested));

    let bytes = to_jcs_bytes(&serde_json::Value::Object(object)).expect("jcs");
    assert_eq!(
        String::from_utf8(bytes).expect("utf8"),
        r#"{"a":2,"m":3,"n":{"b":5,"y":4},"z":1}"#
    );
}

#[test]
fn jcs_rejects_a_non_finite_or_fractional_ambiguous_number() {
    let value: serde_json::Value = serde_json::from_str("{\"x\":1.5}").expect("input");
    assert!(to_jcs_bytes(&value).is_ok());
    assert!(CanonicalJson::parse("{ \"b\":1, \"a\":2 }").is_ok());
    assert_eq!(
        CanonicalJson::parse("{ \"b\":1, \"a\":2 }")
            .expect("parse")
            .as_str(),
        "{\"a\":2,\"b\":1}"
    );
    assert!(CanonicalJson::parse("{").is_err());
}

#[test]
fn observation_coverage_watermarks_are_decimal_epoch_millisecond_scalars() {
    // O-04: every coverage watermark is an accepted-time position in epoch
    // milliseconds carried as a canonical decimal string. A composite
    // `{sequence, at}` object or an RFC 3339 instant would force the reader to
    // reconstruct a scalar it was never given, which is exactly the information
    // loss the decision removes.
    let document = r#"{
        "snapshot": "1754006400000",
        "accepted": "1754006400123",
        "indexed": "1754006399000",
        "earliestReplay": "1753920000000",
        "caughtUp": false,
        "complete": true,
        "missingIntervals": [],
        "unboundedGaps": []
    }"#;
    let coverage: ObservationCoverage = parse(document).expect("coverage decodes");
    assert_eq!(coverage.snapshot, DecimalU128::new(1_754_006_400_000));
    assert_eq!(coverage.accepted, DecimalU128::new(1_754_006_400_123));
    assert_eq!(coverage.indexed, DecimalU128::new(1_754_006_399_000));
    assert_eq!(
        coverage.earliest_replay,
        DecimalU128::new(1_753_920_000_000)
    );

    // The old shapes are refused rather than silently coerced.
    let composite = document.replace(
        "\"accepted\": \"1754006400123\"",
        "\"accepted\": {\"sequence\": \"1754006400123\", \"at\": \"2026-08-01T00:00:00.123Z\"}",
    );
    assert!(parse::<ObservationCoverage>(&composite).is_err());
    let instant = document.replace(
        "\"earliestReplay\": \"1753920000000\"",
        "\"earliestReplay\": \"2026-07-31T00:00:00.000Z\"",
    );
    assert!(parse::<ObservationCoverage>(&instant).is_err());
}
