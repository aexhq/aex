//! What a `regional-work` row, its index and its `NEW_IMAGE` stream may carry.
//!
//! This table's stream duplicates every written row into stream storage and into
//! the pipe role's blast radius. `NEW_IMAGE` is only defensible because the row
//! is defined to hold typed non-content metadata, so the cases below are what
//! turn that definition into a property.

mod support;

use aex_work_dynamodb::codec::{Payload, encode_work, payload_schema};
use aex_work_dynamodb::keys;

use support::record;

fn table_definition() -> serde_json::Value {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../migrations/regional/tables/regional-work.json"),
    )
    .expect("the generation definition is checked in");
    serde_json::from_str(&text).expect("the definition is JSON")
}

#[test]
fn no_declared_payload_schema_admits_a_content_bearing_member() {
    const FORBIDDEN: [&str; 9] = [
        "prompt",
        "body",
        "bodyInline",
        "contentInline",
        "ciphertext",
        "secret",
        "apiKey",
        "url",
        "token",
    ];
    for kind in keys::KINDS {
        let declared = payload_schema(kind).expect("every kind declares a schema");
        for member in declared {
            assert!(
                !FORBIDDEN.contains(member),
                "kind `{kind}` declares payload member `{member}`"
            );
            assert!(
                !member.to_ascii_lowercase().contains("secret"),
                "kind `{kind}` declares payload member `{member}`"
            );
        }
    }
}

/// Every attribute an encoded work row may carry. The `NEW_IMAGE` stream
/// publishes all of them, so the list is exhaustive by intent.
const PERMITTED: [&str; 24] = [
    "pk",
    "sk",
    "itemType",
    "workId",
    "workspaceId",
    "organizationId",
    "sessionId",
    "agentId",
    "kind",
    "priority",
    "dueAt",
    "effectiveDueAt",
    "state",
    "attempt",
    "maxAttempts",
    "fence",
    "claimOwner",
    "leaseExpiresAt",
    "dedupeKey",
    "payload",
    "publishCount",
    "createdAt",
    "updatedAt",
    "lastPublishedAt",
];

#[test]
fn an_encoded_row_carries_no_attribute_outside_the_declared_set() {
    let encoded = encode_work(&record()).expect("encodes");
    for attribute in encoded.keys() {
        assert!(
            PERMITTED.contains(&attribute.as_str())
                || attribute == keys::DUE_PK
                || attribute == keys::DUE_SK
                || attribute == "lastReceiptId",
            "an encoded work row carries `{attribute}`, which the stream would publish"
        );
    }
}

#[test]
fn the_due_index_projection_carries_no_payload() {
    let definition = table_definition();
    let projected: Vec<&str> = definition["globalSecondaryIndexes"][0]["projection"]["attributes"]
        .as_array()
        .expect("an exhaustive attribute list")
        .iter()
        .map(|value| value.as_str().expect("a name"))
        .collect();
    assert!(
        !projected.contains(&"payload"),
        "a due scan must not be able to read a payload"
    );
    assert!(!projected.contains(&"dedupeKey"));
    assert_eq!(
        definition["globalSecondaryIndexes"][0]["projection"]["type"].as_str(),
        Some("INCLUDE")
    );
    assert_eq!(
        definition["globalSecondaryIndexes"][0]["sparse"].as_bool(),
        Some(true)
    );
}

#[test]
fn the_payload_ceiling_is_exact_at_the_boundary() {
    let ceiling = keys::MAX_PAYLOAD_BYTES;
    let name = "fromSeq";
    let exactly = Payload::new().set(name, "x".repeat(ceiling - name.len()));
    assert!(exactly.check("agent.wake").is_ok());
    let one_over = Payload::new().set(name, "x".repeat(ceiling - name.len() + 1));
    assert!(one_over.check("agent.wake").is_err());
}

#[test]
fn only_the_settled_work_row_is_reclaimed_by_ttl() {
    let definition = table_definition();
    let applies_to: Vec<&str> = definition["timeToLive"]["appliesTo"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|value| value.as_str().expect("an item type"))
        .collect();
    assert_eq!(applies_to, vec!["work"]);
    assert_eq!(
        definition["timeToLive"]["attribute"].as_str(),
        Some("expiresAtEpochSeconds")
    );
}
