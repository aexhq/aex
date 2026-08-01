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
            if grant
                .actions
                .iter()
                .any(|action| action == "dynamodb:DeleteItem")
            {
                assert!(
                    matches!(
                        (table.table.as_str(), grant.role.as_str()),
                        ("session-authority", "session-operation-worker")
                            | ("regional-work", "session-operation-worker")
                            | ("regional-content", "content-lifecycle-worker")
                            | ("regional-registry", "regional-session-api")
                            // Each usage worker deletes exactly one row shape:
                            // its own OUTBOX# marker, once the SendMessage is
                            // confirmed. No fact, claim, receipt, frontier or
                            // cursor is deletable by any of them.
                            | ("usage-storage-authority", "usage-storage-worker")
                            | ("usage-compute-authority", "usage-compute-worker")
                            | ("usage-transfer-authority", "usage-transfer-worker")
                    ),
                    "`{}` grants DeleteItem to `{}`, which is not on the deletion allow list",
                    table.table,
                    grant.role
                );
            }
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
