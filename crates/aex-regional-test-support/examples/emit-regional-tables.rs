//! Regenerate `migrations/regional/generated/regional-tables.json`.
//!
//! ```text
//! cargo run -p aex-regional-test-support --example emit-regional-tables
//! ```
//!
//! The example writes the file in place. The bundle is deterministic, so a run
//! that changes nothing produces a clean `git diff`, and
//! `the_generated_bundle_matches_a_deterministic_rebuild` fails whenever the
//! checked-in file drifts from the definitions.

fn main() {
    let bundle = aex_regional_test_support::tables::rebuild()
        .unwrap_or_else(|error| panic!("the regional table definitions do not load: {error}"));
    let path = aex_regional_test_support::tables::bundle_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::fs::write(&path, aex_regional_test_support::tables::render(&bundle))
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    println!("{} tables, digest {}", bundle.tables.len(), bundle.digest);
}
