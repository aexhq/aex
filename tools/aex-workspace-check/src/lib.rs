//! `aex-workspace-check` owns the structural rules of the AEX Cargo workspace:
//! the member list and the on-disk tree agree, names are consistent, dependency
//! direction points inward, no test-support crate can reach a production binary,
//! the member graph is acyclic, and the frozen inventory is exactly present.
//!
//! It reads `cargo metadata`, which is the native manifest graph, and never
//! maintains a second hand-written dependency graph (`M12`).
//!
//! # Invariants
//!
//! - every rule is a pure function of parsed metadata plus a directory listing,
//!   so each is unit-tested against a fixture
//! - a violation names the exact package and the exact reason; there is no
//!   "workspace is invalid" without a fixable detail
//! - a missing prerequisite is a failure, never a skip
//!
//! # Not this crate's job
//!
//! - compiling, testing or linting anything: `cargo` already does that
//! - release graph, artifact or manifest mechanics; those belong to
//!   `tools/aex-release-tool`
//! - product policy of any kind

pub mod inventory;
pub mod metadata;
pub mod rules;

use std::path::Path;

pub use metadata::{MetadataError, WorkspaceMetadata};
pub use rules::{TreeListing, Violation, Workspace};

/// Why the workspace could not be checked.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// `cargo metadata` output could not be used.
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    /// A member root could not be read.
    #[error("cannot read `{root}`: {source}")]
    Tree {
        /// The root that could not be listed.
        root: String,
        /// The underlying I/O failure.
        source: std::io::Error,
    },
}

/// Lists the directories under each member root of `workspace_root`.
///
/// A root that does not exist yields no entries; a root that exists but cannot
/// be read is an error, because silently treating it as empty would turn a
/// permission problem into a passing check.
///
/// # Errors
///
/// Returns [`CheckError::Tree`] when an existing root cannot be listed.
pub fn read_tree(workspace_root: &Path) -> Result<TreeListing, CheckError> {
    let mut listing = TreeListing::new();
    for root in inventory::MEMBER_ROOTS {
        let path = root
            .split('/')
            .fold(workspace_root.to_path_buf(), |accumulator, segment| {
                accumulator.join(segment)
            });
        if !path.is_dir() {
            listing.insert((*root).to_owned(), Vec::new());
            continue;
        }
        let entries = std::fs::read_dir(&path).map_err(|source| CheckError::Tree {
            root: (*root).to_owned(),
            source,
        })?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| CheckError::Tree {
                root: (*root).to_owned(),
                source,
            })?;
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        listing.insert((*root).to_owned(), names);
    }
    Ok(listing)
}

/// Parses `cargo metadata` output, reads the tree it describes, and runs every
/// rule.
///
/// # Errors
///
/// Returns [`CheckError`] when the metadata cannot be parsed or a member root
/// cannot be read.
pub fn check_metadata_json(json: &str) -> Result<Vec<Violation>, CheckError> {
    let metadata = WorkspaceMetadata::parse(json)?;
    let tree = read_tree(Path::new(&metadata.workspace_root))?;
    Ok(rules::check(&Workspace { metadata, tree }))
}

#[cfg(test)]
mod tests {
    use super::read_tree;
    use crate::inventory::MEMBER_ROOTS;
    use std::path::Path;

    #[test]
    fn a_missing_root_lists_no_entries_rather_than_failing() {
        let listing = read_tree(Path::new("this-workspace-root-does-not-exist"))
            .expect("a missing root is not an error");
        assert_eq!(listing.len(), MEMBER_ROOTS.len());
        assert!(listing.values().all(Vec::is_empty));
    }
}
