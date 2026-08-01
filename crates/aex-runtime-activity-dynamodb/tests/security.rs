//! Least-visibility cases for `runtime-activity`.
//!
//! This table's change feed carries a `NEW_IMAGE`, which is only safe because
//! the rows hold generation metadata and nothing else. These cases are what keep
//! that true.

mod support;

use aex_runtime_activity_dynamodb::codec::{
    encode_generation, encode_intent, encode_probe, encode_receipt,
};
use aex_runtime_activity_dynamodb::keys;
use aex_runtime_control::generation::GenerationState;

use support::{DEFINITION, head, intent, probe, receipt, session, workspace};

const FORBIDDEN: &[&str] = &[
    "prompt", "body", "cipher", "secret", "token", "content", "message",
];

#[test]
fn no_row_this_table_holds_has_an_attribute_that_could_carry_customer_content() {
    let rows = [
        encode_generation(&head(GenerationState::Running)).expect("encodes"),
        encode_intent(&intent()).expect("encodes"),
        encode_receipt(&receipt()).expect("encodes"),
        encode_probe(&probe()),
    ];
    for row in rows {
        for name in row.keys() {
            let lowered = name.to_lowercase();
            for forbidden in FORBIDDEN {
                assert!(
                    !lowered.contains(forbidden),
                    "`{name}` would put `{forbidden}` on a NEW_IMAGE stream"
                );
            }
        }
    }
}

#[test]
fn the_due_projection_carries_identities_and_schedule_facts_only() {
    for attribute in keys::DUE_PROJECTION {
        let lowered = attribute.to_lowercase();
        for forbidden in FORBIDDEN {
            assert!(
                !lowered.contains(forbidden),
                "the reaper's scan would read `{attribute}`"
            );
        }
    }
    let definition: serde_json::Value =
        serde_json::from_str(DEFINITION).expect("the generation definition is JSON");
    let declared: Vec<String> = definition["globalSecondaryIndexes"][0]["projection"]["attributes"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|entry| entry.as_str().expect("a string").to_owned())
        .collect();
    assert_eq!(declared, keys::DUE_PROJECTION);
}

#[test]
fn the_pointer_a_brain_activation_reads_lives_outside_the_generation_partition() {
    assert_ne!(
        keys::current(session()).pk,
        keys::head(session(), support::generation(4)).pk
    );
}

#[test]
fn a_generation_row_names_its_workspace_so_a_read_can_be_re_checked() {
    let encoded = encode_generation(&head(GenerationState::Running)).expect("encodes");
    assert_eq!(
        encoded["workspaceId"].as_s().expect("a workspace"),
        &workspace().to_string(),
        "IAM separates roles, not tenants; the post-read check is what separates tenants"
    );
}
