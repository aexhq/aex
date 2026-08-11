//! Cross-module properties for the immutable launch catalog.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use aex_brain_tool_catalog::catalog::{
    BUILTIN_CATALOG_DIGEST, builtin_catalog_bytes, builtin_entries,
};
use sha2::{Digest as _, Sha256};

#[test]
fn builtin_catalog_has_one_stable_row_per_launch_tool() {
    let entries = builtin_entries().expect("compiled catalog must be valid");
    let names = entries
        .iter()
        .map(|entry| entry.descriptor.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["bash", "edit_file", "read_file", "write_file"])
    );
    assert_eq!(entries.len(), 4);

    let digest = Sha256::digest(builtin_catalog_bytes().expect("catalog must canonicalize"));
    let mut actual = "sha256:".to_owned();
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing to a string cannot fail");
    }
    assert_eq!(actual, BUILTIN_CATALOG_DIGEST);
}
