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
                    "session-authority" | "regional-work" => {
                        grant.role == "session-operation-worker"
                    }
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
                    // Runtime activity has two narrow deleters: the control
                    // worker removes usage outbox rows, while Brain settles its
                    // own Hands admission marker.
                    "runtime-activity" => {
                        matches!(grant.role.as_str(), "runtime-control-worker" | "brain-mux")
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
