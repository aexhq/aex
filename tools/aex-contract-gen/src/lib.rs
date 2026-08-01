//! The deterministic AEX contract generator.
//!
//! One authored tree under `api/` produces every downstream artifact: the two
//! `OpenAPI` 3.1 plane documents, the published JSON Schemas, the machine
//! registries, the contract bundle and its lock file, and the generated Rust
//! surface of `aex-wire`. `build` and `check` share one code path and differ
//! only in whether the finished tree is written or compared, which is what makes
//! "the committed output matches a fresh run" a real assertion.
//!
//! # Determinism rules this crate holds itself to
//!
//! - no unordered iteration: `BTreeMap`, `BTreeSet` and `Vec` only;
//! - no ambient input: no clock, no RNG, no environment read, no file metadata;
//! - a sorted, symlink-free, extension-filtered walk for file discovery;
//! - every recorded path is workspace-relative with `/` separators, so a Windows
//!   run and a Linux run produce identical bytes.

#![allow(
    clippy::too_many_lines,
    reason = "an emitter is one linear render; splitting it hides the shape it produces"
)]
#![allow(
    clippy::format_push_string,
    reason = "a renderer reads better with `format!` than with a `fmt::Write` import"
)]

pub mod classify;
pub mod emit;
pub mod emit_models;
pub mod emit_routes;
pub mod error;
pub mod ir;
pub mod jcs;
pub mod load;
pub mod rustsrc;
pub mod tree;

use std::path::Path;

pub use error::GenError;
pub use tree::GeneratedTree;

/// Loads the authored tree and renders every output into memory.
///
/// # Errors
///
/// Returns [`GenError`] for any structural failure in the authored tree.
pub fn generate_to_memory(root: &Path) -> Result<GeneratedTree, GenError> {
    let ir = load::load(root)?;
    Ok(emit::emit_all(&ir))
}

/// Renders and writes every output.
///
/// # Errors
///
/// Returns [`GenError`] for a load failure, or an I/O failure naming the file.
pub fn build(root: &Path) -> Result<GeneratedTree, GenError> {
    let tree = generate_to_memory(root)?;
    tree.write_to(root).map_err(|reason| GenError::Io {
        path: root.display().to_string(),
        reason,
    })?;
    Ok(tree)
}

/// Renders every output and compares it against what is committed.
///
/// # Errors
///
/// Returns [`GenError`] for a load failure. Drift is returned as data, not as an
/// error, so a caller can print every offending file at once.
pub fn check(root: &Path) -> Result<Vec<String>, GenError> {
    let tree = generate_to_memory(root)?;
    Ok(tree.diff_against_disk(root))
}
