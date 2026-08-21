//! Aex-owned control-plane schema and generated-type conformance.

use std::path::{Path, PathBuf};

use aex_contracts::control;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn examples() -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(repo_root().join("contracts/examples/control")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let type_name = name.split('.').next().unwrap().to_string();
        let value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        out.push((name, type_name, value));
    }
    assert!(!out.is_empty(), "no Aex control examples");
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn validator(type_name: &str) -> jsonschema::Validator {
    let mut schema: Value = serde_json::from_str(aex_contracts::CONTROL_SCHEMA_JSON).unwrap();
    schema
        .as_object_mut()
        .unwrap()
        .insert("$ref".into(), Value::String(format!("#/$defs/{type_name}")));
    schema.as_object_mut().unwrap().remove("$id");
    jsonschema::draft202012::new(&schema)
        .unwrap_or_else(|error| panic!("invalid schema for {type_name}: {error}"))
}

fn round_trip<T: Serialize + DeserializeOwned>(name: &str, value: &Value) {
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|error| {
        panic!(
            "{name}: does not deserialize into {}: {error}",
            std::any::type_name::<T>()
        )
    });
    let back = serde_json::to_value(typed).unwrap();
    assert_lossless(value, &back, name);
}

fn assert_lossless(expected: &Value, actual: &Value, path: &str) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                match actual.get(key) {
                    Some(actual) => assert_lossless(value, actual, &format!("{path}/{key}")),
                    None if value.is_null() => {}
                    None => panic!("{path}/{key}: missing after round trip"),
                }
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(expected.len(), actual.len(), "{path}: array length changed");
            for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                assert_lossless(expected, actual, &format!("{path}/{index}"));
            }
        }
        _ => assert_eq!(expected, actual, "{path}: value changed"),
    }
}

#[test]
fn schema_is_valid_2020_12() {
    let schema: Value = serde_json::from_str(aex_contracts::CONTROL_SCHEMA_JSON).unwrap();
    jsonschema::meta::validate(&schema).unwrap();
}

#[test]
fn examples_validate_and_round_trip() {
    for (name, type_name, value) in examples() {
        let errors: Vec<_> = validator(&type_name)
            .iter_errors(&value)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{name}: {}", errors.join("; "));
        match type_name.as_str() {
            "JoinWaitlistRequest" => round_trip::<control::JoinWaitlistRequest>(&name, &value),
            "WaitlistSubmission" => round_trip::<control::WaitlistSubmission>(&name, &value),
            "WaitlistEntry" => round_trip::<control::WaitlistEntry>(&name, &value),
            "WaitlistEntryList" => round_trip::<control::WaitlistEntryList>(&name, &value),
            "CreateInvitationRequest" => {
                round_trip::<control::CreateInvitationRequest>(&name, &value)
            }
            "InvitationCreated" => round_trip::<control::InvitationCreated>(&name, &value),
            "Account" => round_trip::<control::Account>(&name, &value),
            "CreateAccountRequest" => round_trip::<control::CreateAccountRequest>(&name, &value),
            "AccountCreated" => round_trip::<control::AccountCreated>(&name, &value),
            "ApiKey" => round_trip::<control::ApiKey>(&name, &value),
            "CreateApiKeyRequest" => round_trip::<control::CreateApiKeyRequest>(&name, &value),
            "ApiKeyCreated" => round_trip::<control::ApiKeyCreated>(&name, &value),
            "ApiKeyList" => round_trip::<control::ApiKeyList>(&name, &value),
            "Balance" => round_trip::<control::Balance>(&name, &value),
            "CreateTopupRequest" => round_trip::<control::CreateTopupRequest>(&name, &value),
            "Topup" => round_trip::<control::Topup>(&name, &value),
            "TopupList" => round_trip::<control::TopupList>(&name, &value),
            "CreateCreditGrantRequest" => {
                round_trip::<control::CreateCreditGrantRequest>(&name, &value)
            }
            "CreditGrant" => round_trip::<control::CreditGrant>(&name, &value),
            "CreateRefundRequest" => round_trip::<control::CreateRefundRequest>(&name, &value),
            "Refund" => round_trip::<control::Refund>(&name, &value),
            "RateCard" => round_trip::<control::RateCard>(&name, &value),
            "SessionUsage" => round_trip::<control::SessionUsage>(&name, &value),
            "Usage" => round_trip::<control::Usage>(&name, &value),
            "ControlErrorResponse" => round_trip::<control::ControlErrorResponse>(&name, &value),
            other => panic!("{name}: no round-trip mapping for control type {other}"),
        }
    }
}

#[test]
fn exact_integer_strings_are_canonical_and_numeric_fields_are_bounded() {
    let signed = validator("MicroUsd");
    for valid in [
        "0",
        "1",
        "-1",
        "9223372036854775807",
        "-9223372036854775808",
    ] {
        assert!(
            signed.is_valid(&serde_json::json!(valid)),
            "rejected signed {valid}"
        );
    }
    for invalid in ["", "-0", "+1", "00", "01", "-01", "1.0", "1e3", " 1"] {
        assert!(
            !signed.is_valid(&serde_json::json!(invalid)),
            "accepted non-canonical signed {invalid:?}"
        );
    }
    assert!(!signed.is_valid(&serde_json::json!(1)));

    let unsigned = validator("UnsignedDecimalInteger");
    for valid in ["0", "1", "9007199254740992", "9223372036854775807"] {
        assert!(
            unsigned.is_valid(&serde_json::json!(valid)),
            "rejected unsigned {valid}"
        );
    }
    for invalid in ["", "-0", "-1", "+1", "00", "01", "1.0", "1e3", " 1"] {
        assert!(
            !unsigned.is_valid(&serde_json::json!(invalid)),
            "accepted non-canonical unsigned {invalid:?}"
        );
    }
    assert!(!unsigned.is_valid(&serde_json::json!(0)));

    let limits = validator("AccountLimits");
    assert!(limits.is_valid(&serde_json::json!({
        "max_concurrent_sessions": 1,
        "session_creates_per_hour": 1_000_000
    })));
    assert!(!limits.is_valid(&serde_json::json!({
        "max_concurrent_sessions": 0,
        "session_creates_per_hour": 1
    })));
    assert!(!limits.is_valid(&serde_json::json!({
        "max_concurrent_sessions": 1,
        "session_creates_per_hour": 1_000_001
    })));

    let topup = validator("CreateTopupRequest");
    assert!(topup.is_valid(&serde_json::json!({"amount_cents": 1_000})));
    assert!(topup.is_valid(&serde_json::json!({"amount_cents": 100_000})));
    assert!(!topup.is_valid(&serde_json::json!({"amount_cents": 999})));
    assert!(!topup.is_valid(&serde_json::json!({"amount_cents": 100_001})));

    let storage = validator("StorageMeters");
    assert!(storage.is_valid(&serde_json::json!({
        "session_storage_bytes": 10_737_418_240u64,
        "upload_reserved_bytes": 0
    })));
    assert!(!storage.is_valid(&serde_json::json!({
        "session_storage_bytes": 10_737_418_241u64,
        "upload_reserved_bytes": 0
    })));
}
