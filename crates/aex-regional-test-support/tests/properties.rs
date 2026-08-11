//! Determinism properties for the regional fixtures and the table-definition
//! loader.
//!
//! The fixture properties prove the seeded factory is a function of its seed.
//! The loader properties prove the generated bundle is a function of the
//! definition files, which is what lets Terraform consume a checked-in artefact
//! without a build step.

use aex_regional_test_support::tables;
use proptest::prelude::*;

#[test]
fn the_bundle_is_a_pure_function_of_the_definition_files() {
    let first = tables::rebuild().expect("the definitions load");
    let second = tables::rebuild().expect("the definitions load");
    assert_eq!(tables::render(&first), tables::render(&second));
}

#[test]
fn every_table_declares_at_least_one_role_and_no_role_holds_a_delete_it_does_not_need() {
    for table in tables::rebuild().expect("the definitions load").tables {
        assert!(
            !table.iam.is_empty(),
            "{} grants nobody access",
            table.table
        );
        for grant in &table.iam {
            // `BatchWriteItem` deletes as well as writes, so it is checked
            // against the same allow list; a delete vector nobody listed is a
            // delete vector nobody reviewed.
            if grant.actions.iter().any(|action| {
                action == "dynamodb:DeleteItem" || action == "dynamodb:BatchWriteItem"
            }) {
                let allowed = match table.table.as_str() {
                    // Session deletion owns terminal cleanup. Regional control
                    // removes only stale active-session locators; a focused
                    // table test proves its item and leading-key restriction.
                    "session-authority" => matches!(
                        grant.role.as_str(),
                        "session-operation-worker" | "regional-control"
                    ),
                    "regional-work" => grant.role == "session-operation-worker",
                    "regional-content" => grant.role == "content-lifecycle-worker",
                    // The registry API deletes registered names outright. The
                    // upload-expiry collector is the second, far narrower
                    // deleter: it reclaims abandoned multipart uploads and is
                    // confined by a leading-key condition to `UPLOAD#*` and to
                    // the two upload row shapes, so it cannot reach a
                    // registered name.
                    "regional-registry" => matches!(
                        grant.role.as_str(),
                        "regional-session-api" | "content-lifecycle-worker-uploadexpiry"
                    ),
                    // Each usage worker deletes exactly one row shape: its own
                    // OUTBOX# marker, once SendMessage is confirmed.
                    "usage-storage-authority" => grant.role == "usage-storage-worker",
                    "usage-compute-authority" => grant.role == "usage-compute-worker",
                    "usage-transfer-authority" => grant.role == "usage-transfer-worker",
                    // Runtime activity has three narrow deleters: the control
                    // worker removes usage outbox rows, Brain settles its own
                    // Hands admission marker, and session deletion removes
                    // only an already-terminal exact generation.
                    "runtime-activity" => {
                        matches!(
                            grant.role.as_str(),
                            "runtime-control-worker" | "brain-mux" | "session-operation-worker"
                        )
                    }
                    // The reconciler deletes exactly two shapes: an idle series
                    // claim and OBS# revisions under a pinned deletion epoch.
                    "observation-authority" => {
                        matches!(
                            grant.role.as_str(),
                            "observation-reconciler" | "regional-otlp"
                        )
                    }
                    _ => false,
                };
                assert!(
                    allowed,
                    "`{}` grants a delete vector to `{}`, which is not on the deletion allow list",
                    table.table, grant.role
                );
            }
        }
    }
}

#[test]
fn session_delete_worker_can_remove_only_one_sessions_terminal_runtime_rows() {
    let bundle = tables::rebuild().expect("the definitions load");
    let runtime = bundle
        .tables
        .iter()
        .find(|table| table.table == "runtime-activity")
        .expect("runtime activity");
    let grants = runtime
        .iam
        .iter()
        .filter(|grant| grant.role == "session-operation-worker")
        .collect::<Vec<_>>();
    assert_eq!(grants.len(), 2, "read and deletion grants stay disjoint");
    let grant = grants
        .into_iter()
        .find(|grant| {
            grant
                .actions
                .iter()
                .any(|action| action == "dynamodb:DeleteItem")
        })
        .expect("one deletion grant");
    assert_eq!(
        grant.actions,
        [
            "dynamodb:GetItem",
            "dynamodb:Query",
            "dynamodb:DeleteItem",
            "dynamodb:TransactWriteItems",
        ]
    );
    assert_eq!(grant.resources, ["table"]);
    assert_eq!(
        grant.item_types,
        [
            "hands_generation",
            "lifecycle_intent",
            "lifecycle_receipt",
            "idle_probe",
            "current_generation",
            "hands_operation_admission",
        ]
    );
    assert!(!grant.item_types.iter().any(|item| item == "usage_outbox"));
    let condition = grant.condition.as_ref().expect("leading-key fence");
    assert_eq!(condition.operator, "ForAllValues:StringLike");
    assert_eq!(condition.key, "dynamodb:LeadingKeys");
    assert_eq!(condition.values, ["GEN#*", "SESSIONGEN#*"]);
}

#[test]
fn session_operation_worker_holds_only_the_observation_deletion_control_actions() {
    let bundle = tables::rebuild().expect("the definitions load");
    let observation = bundle
        .tables
        .iter()
        .find(|table| table.table == "observation-authority")
        .expect("observation authority");
    let grants = observation
        .iam
        .iter()
        .filter(|grant| grant.role == "session-operation-worker")
        .collect::<Vec<_>>();
    assert_eq!(grants.len(), 1);
    assert_eq!(
        grants[0].actions,
        ["dynamodb:GetItem", "dynamodb:UpdateItem"]
    );
    assert_eq!(grants[0].resources, ["table"]);
    assert_eq!(grants[0].item_types, ["scope_deletion"]);
    let condition = grants[0].condition.as_ref().expect("leading-key fence");
    assert_eq!(condition.operator, "ForAllValues:StringLike");
    assert_eq!(condition.key, "dynamodb:LeadingKeys");
    assert_eq!(condition.values, ["FRONT#*"]);
}

#[test]
fn every_manifest_uses_real_dynamodb_iam_actions() {
    for table in tables::rebuild().expect("the definitions load").tables {
        for grant in table.iam {
            assert!(
                !grant
                    .actions
                    .iter()
                    .any(|action| action == "dynamodb:ConditionCheckItem"),
                "{} grants nonexistent ConditionCheckItem to {}; transaction condition checks require dynamodb:TransactWriteItems",
                table.table,
                grant.role
            );
        }
    }
}

#[test]
fn the_session_operation_worker_can_read_only_exact_runtime_generations() {
    let tables = tables::rebuild().expect("the definitions load");
    let runtime = tables
        .tables
        .iter()
        .find(|table| table.table == "runtime-activity")
        .expect("runtime activity is declared");
    let grants = runtime
        .iam
        .iter()
        .filter(|grant| grant.role == "session-operation-worker")
        .collect::<Vec<_>>();

    assert_eq!(
        grants.len(),
        2,
        "the lifecycle read and deletion grants stay independently fenced"
    );
    let grant = grants
        .into_iter()
        .find(|grant| grant.actions.as_slice() == ["dynamodb:GetItem"])
        .expect("the lifecycle bridge has one exact-generation read grant");
    assert_eq!(grant.actions, ["dynamodb:GetItem"]);
    assert_eq!(grant.resources, ["table"]);
    assert!(
        grant.item_types.is_empty(),
        "a read grant owns no row family"
    );
    let condition = grant
        .condition
        .as_ref()
        .expect("the read is restricted to generation partitions");
    assert_eq!(condition.operator, "ForAllValues:StringLike");
    assert_eq!(condition.key, "dynamodb:LeadingKeys");
    assert_eq!(condition.values, ["GEN#*"]);
}

#[test]
fn only_the_keystore_departs_from_the_pk_sk_convention() {
    for table in tables::rebuild().expect("the definitions load").tables {
        if table.table == "regional-secret-keystore" {
            assert_eq!(table.key_schema.partition, "branch-key-id");
            assert_eq!(table.key_schema.sort, "type");
            continue;
        }
        assert_eq!(table.key_schema.partition, "pk", "{}", table.table);
        assert_eq!(table.key_schema.sort, "sk", "{}", table.table);
    }
}

proptest! {
    #[test]
    fn a_seeded_identifier_factory_is_a_function_of_its_seed(seed in any::<u64>(), draws in 1_usize..24) {
        let mut left = aex_regional_test_support::IdFactory::new(seed);
        let mut right = aex_regional_test_support::IdFactory::new(seed);
        for _ in 0..draws {
            prop_assert_eq!(left.next_id(), right.next_id());
        }
    }

    #[test]
    fn identifiers_from_one_factory_are_strictly_increasing(seed in any::<u64>(), draws in 2_usize..24) {
        let mut factory = aex_regional_test_support::IdFactory::new(seed);
        let mut previous = factory.next_id();
        for _ in 1..draws {
            let next = factory.next_id();
            prop_assert!(next > previous, "{next} did not follow {previous}");
            previous = next;
        }
    }
}
