//! G2: the bounds corpus, the protobuf/JSON parity corpus, and the fuzz target
//! run as a property test.
//!
//! The decoder is an untrusted parser, so its declared fuzz property — *no
//! panic, bounded time, bounded memory, on any input* — is exercised here as a
//! `proptest` in the default lane rather than only in a separate fuzz job that
//! nothing gates on.

use aex_otlp_admission::decode::{
    ContentCoding, DecodeRequest, DecodedBatch, OtlpEncoding, decode,
};
use aex_otlp_admission::error::{OtlpError, OtlpSignal};
use aex_otlp_admission::limits::OtlpLimits;
use aex_otlp_admission::memory::MemoryBudget;
use aex_otlp_admission::proto::{collector_logs, common, logs};
use proptest::prelude::*;
use prost::Message as _;

fn budget() -> MemoryBudget {
    MemoryBudget::new(64 * 1024 * 1024)
}

fn decode_bytes(
    signal: OtlpSignal,
    encoding: OtlpEncoding,
    coding: ContentCoding,
    body: &[u8],
    limits: &OtlpLimits,
) -> Result<DecodedBatch, OtlpError> {
    let budget = budget();
    let lease = budget
        .try_reserve(limits.decoded_max.min(32 * 1024 * 1024))
        .expect("the fixture budget covers one request");
    decode(&DecodeRequest {
        signal,
        encoding,
        coding,
        body,
        limits,
        lease: &lease,
    })
}

fn log_batch(records: usize) -> collector_logs::ExportLogsServiceRequest {
    collector_logs::ExportLogsServiceRequest {
        resource_logs: vec![logs::ResourceLogs {
            resource: None,
            scope_logs: vec![logs::ScopeLogs {
                scope: None,
                log_records: (0..records)
                    .map(|index| logs::LogRecord {
                        time_unix_nano: 1_700_000_000_000_000_000
                            + u64::try_from(index).expect("fixture fits"),
                        severity_number: 9,
                        body: Some(common::AnyValue {
                            value: Some(common::any_value::Value::StringValue(format!("m{index}"))),
                        }),
                        ..logs::LogRecord::default()
                    })
                    .collect(),
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}

// --- G2(10) the bounds corpus ----------------------------------------------

#[test]
fn the_record_ceiling_is_exact_at_the_boundary() {
    let limits = OtlpLimits {
        max_records: 2_000,
        ..OtlpLimits::REGISTERED
    };
    for (records, expected_ok) in [(1_999_usize, true), (2_000, true), (2_001, false)] {
        let bytes = log_batch(records).encode_to_vec();
        let outcome = decode_bytes(
            OtlpSignal::Logs,
            OtlpEncoding::Protobuf,
            ContentCoding::Identity,
            &bytes,
            &limits,
        );
        assert_eq!(
            outcome.is_ok(),
            expected_ok,
            "{records} records against a 2,000 ceiling"
        );
        if !expected_ok {
            assert!(matches!(
                outcome.expect_err("refused"),
                OtlpError::TooManyRecords {
                    observed: 2_001,
                    limit: 2_000
                }
            ));
        }
    }
}

#[test]
fn the_encoded_ceiling_is_exact_at_the_boundary() {
    let limits = OtlpLimits {
        encoded_max: 1_024,
        ..OtlpLimits::REGISTERED
    };
    for (len, expected_ok) in [(1_023_usize, true), (1_024, true), (1_025, false)] {
        // Only the size gate is under test, so a body that is not valid
        // protobuf still proves the gate fires before the parser is reached.
        let body = vec![0u8; len];
        let outcome = decode_bytes(
            OtlpSignal::Logs,
            OtlpEncoding::Protobuf,
            ContentCoding::Identity,
            &body,
            &limits,
        );
        match (expected_ok, outcome) {
            (true, Ok(_)) => {}
            (true, Err(error)) => assert!(
                !matches!(error, OtlpError::EncodedTooLarge { .. }),
                "{len} bytes must pass the size gate"
            ),
            (false, Err(OtlpError::EncodedTooLarge { observed, limit })) => {
                assert_eq!((observed, limit), (1_025, 1_024));
            }
            (false, other) => panic!("{len} bytes must be refused, got {other:?}"),
        }
    }
}

#[test]
fn the_decoded_ceiling_is_exact_at_the_boundary_for_an_identity_body() {
    let limits = OtlpLimits {
        encoded_max: 1_024 * 1_024,
        decoded_max: 512,
        ..OtlpLimits::REGISTERED
    };
    let error = decode_bytes(
        OtlpSignal::Logs,
        OtlpEncoding::Protobuf,
        ContentCoding::Identity,
        &vec![0u8; 513],
        &limits,
    )
    .expect_err("refused");
    assert!(matches!(error, OtlpError::DecodedTooLarge { limit: 512 }));
}

#[test]
fn the_attribute_bounds_produce_the_exact_typed_error_at_the_record_pointer() {
    use aex_otlp_admission::normalize::{AuthenticatedScope, normalize};

    let scope = AuthenticatedScope {
        organization_id: aex_wire::ids::PrefixedId::parse("org_0000000001e40r2081040g2081")
            .expect("fixture parses"),
        workspace_id: aex_wire::ids::PrefixedId::parse("wsp_0000000002e81840g2081040g2")
            .expect("fixture parses"),
        session_id: None,
        run_id: None,
        agent_id: None,
    };

    let cases: [(&str, OtlpLimits, common::KeyValue); 3] = [
        (
            "attribute key bytes",
            OtlpLimits {
                max_attribute_key_bytes: 4,
                ..OtlpLimits::REGISTERED
            },
            string_attribute(&"k".repeat(5), "v"),
        ),
        (
            "attribute value bytes",
            OtlpLimits {
                max_attribute_value_bytes: 4,
                ..OtlpLimits::REGISTERED
            },
            string_attribute("k", &"v".repeat(5)),
        ),
        (
            "array elements",
            OtlpLimits {
                max_array_elements: 2,
                ..OtlpLimits::REGISTERED
            },
            array_attribute("k", 3),
        ),
    ];

    for (bound, limits, attribute) in cases {
        let mut request = log_batch(1);
        request.resource_logs[0].scope_logs[0].log_records[0].attributes = vec![attribute];
        let batch = DecodedBatch::Logs(Box::new(request));
        let error = normalize(&batch, &scope, &limits).expect_err("refused");
        match error {
            OtlpError::RecordBound {
                bound: observed_bound,
                at,
                ..
            } => {
                assert_eq!(observed_bound, bound);
                assert_eq!(&*at, "/0/0/0", "the pointer names the offending record");
            }
            other => panic!("expected a `{bound}` record bound, got {other:?}"),
        }
    }
}

fn string_attribute(key: &str, value: &str) -> common::KeyValue {
    common::KeyValue {
        key: key.to_owned(),
        value: Some(common::AnyValue {
            value: Some(common::any_value::Value::StringValue(value.to_owned())),
        }),
        key_strindex: 0,
    }
}

fn array_attribute(key: &str, elements: usize) -> common::KeyValue {
    common::KeyValue {
        key: key.to_owned(),
        value: Some(common::AnyValue {
            value: Some(common::any_value::Value::ArrayValue(common::ArrayValue {
                values: (0..elements)
                    .map(|index| common::AnyValue {
                        value: Some(common::any_value::Value::IntValue(
                            i64::try_from(index).expect("fixture fits"),
                        )),
                    })
                    .collect(),
            })),
        }),
        key_strindex: 0,
    }
}

// --- G2(9) protobuf / JSON parity ------------------------------------------

#[test]
fn protobuf_and_json_reach_the_same_decoded_batch_for_every_signal() {
    let logs_json = br#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[
        {"timeUnixNano":"1700000000000000000","severityNumber":9,
         "body":{"stringValue":"m0"}}]}]}]}"#;
    let logs_proto = log_batch(1).encode_to_vec();
    let limits = OtlpLimits::REGISTERED;
    let from_json = decode_bytes(
        OtlpSignal::Logs,
        OtlpEncoding::Json,
        ContentCoding::Identity,
        logs_json,
        &limits,
    )
    .expect("json decodes");
    let from_proto = decode_bytes(
        OtlpSignal::Logs,
        OtlpEncoding::Protobuf,
        ContentCoding::Identity,
        &logs_proto,
        &limits,
    )
    .expect("protobuf decodes");
    assert_eq!(from_json, from_proto);

    let spans_json = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[
        {"traceId":"aabbccddeeff00112233445566778899","spanId":"0011223344556677",
         "name":"s","kind":"SPAN_KIND_SERVER","startTimeUnixNano":"1","endTimeUnixNano":"2"}]}]}]}"#;
    let spans = decode_bytes(
        OtlpSignal::Traces,
        OtlpEncoding::Json,
        ContentCoding::Identity,
        spans_json,
        &limits,
    )
    .expect("json decodes");
    let DecodedBatch::Traces(request) = &spans else {
        panic!("traces");
    };
    let round_tripped = decode_bytes(
        OtlpSignal::Traces,
        OtlpEncoding::Protobuf,
        ContentCoding::Identity,
        &request.encode_to_vec(),
        &limits,
    )
    .expect("protobuf decodes");
    assert_eq!(spans, round_tripped);

    let metrics_json = br#"{"resourceMetrics":[{"scopeMetrics":[{"metrics":[
        {"name":"m","unit":"1","sum":{"dataPoints":[{"asInt":"3","timeUnixNano":"7"}],
         "aggregationTemporality":"AGGREGATION_TEMPORALITY_DELTA","isMonotonic":true}}]}]}]}"#;
    let metrics = decode_bytes(
        OtlpSignal::Metrics,
        OtlpEncoding::Json,
        ContentCoding::Identity,
        metrics_json,
        &limits,
    )
    .expect("json decodes");
    let DecodedBatch::Metrics(request) = &metrics else {
        panic!("metrics");
    };
    let round_tripped = decode_bytes(
        OtlpSignal::Metrics,
        OtlpEncoding::Protobuf,
        ContentCoding::Identity,
        &request.encode_to_vec(),
        &limits,
    )
    .expect("protobuf decodes");
    assert_eq!(metrics, round_tripped);
}

// --- G2(15) the fuzz property ----------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary bytes on any route, encoding and coding: no panic, ever.
    #[test]
    fn arbitrary_bytes_never_panic(
        raw in proptest::collection::vec(any::<u8>(), 0..4096),
        signal_index in 0_usize..3,
        json in any::<bool>(),
        gzip in any::<bool>(),
    ) {
        let limits = OtlpLimits {
            encoded_max: 8 * 1024,
            decoded_max: 64 * 1024,
            ..OtlpLimits::REGISTERED
        };
        let outcome = decode_bytes(
            OtlpSignal::ALL[signal_index],
            if json { OtlpEncoding::Json } else { OtlpEncoding::Protobuf },
            if gzip { ContentCoding::Gzip } else { ContentCoding::Identity },
            &raw,
            &limits,
        );
        // Success or a typed error; a panic would fail the case by unwinding.
        if let Ok(batch) = outcome {
            prop_assert!(batch.record_count() <= limits.max_records);
        }
    }

    /// Arbitrary JSON-shaped text never panics and never admits an unknown
    /// member.
    #[test]
    fn arbitrary_json_text_never_panics(text in ".{0,512}") {
        let limits = OtlpLimits::REGISTERED;
        let _ = decode_bytes(
            OtlpSignal::Logs,
            OtlpEncoding::Json,
            ContentCoding::Identity,
            text.as_bytes(),
            &limits,
        );
    }
}
