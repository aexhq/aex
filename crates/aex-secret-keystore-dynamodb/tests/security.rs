//! Least-privilege and write-boundary cases for `regional-secret-keystore`.

mod support;

use support::{BRANCH_KEY_SOURCE, STORE_SOURCE};

#[test]
fn this_crate_contains_no_write_operation_at_all() {
    // D-20: creation and rotation are provider operations. A grep is the only
    // way to assert the absence of a capability at the unit layer, and the
    // absence is the whole design.
    for forbidden in [
        "put_item(",
        "update_item(",
        "delete_item(",
        "transact_write_items(",
        "batch_write_item(",
    ] {
        assert!(
            !STORE_SOURCE.contains(forbidden),
            "`{forbidden}` appeared in the key store reader"
        );
        assert!(
            !BRANCH_KEY_SOURCE.contains(forbidden),
            "`{forbidden}` appeared in the record codec"
        );
    }
}

#[test]
fn the_crate_never_names_a_key_creation_operation_either() {
    for forbidden in [
        "CreateKey",
        "VersionKey",
        "CreateKeyStore",
        "generate_data_key",
    ] {
        assert!(
            !STORE_SOURCE.contains(forbidden) && !BRANCH_KEY_SOURCE.contains(forbidden),
            "`{forbidden}` belongs to regional-secret-key-admin, not here"
        );
    }
}

#[test]
fn the_reader_trait_exposes_reads_and_nothing_else() {
    // The trait is the contract a composition depends on; a write method added
    // later would have to pass this.
    let trait_block = STORE_SOURCE
        .split("pub trait BranchKeyStoreReader")
        .nth(1)
        .expect("the trait is declared")
        .split(
            "
}",
        )
        .next()
        .expect("the trait ends");
    for method in trait_block.split("async fn ").skip(1) {
        let name = method.split('(').next().expect("a method name");
        assert!(
            name.starts_with("describe") || name.starts_with("list"),
            "`{name}` is not a read"
        );
    }
}
