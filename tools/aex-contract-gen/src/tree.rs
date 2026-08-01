//! The in-memory output tree.
//!
//! Every emitter writes here, never to disk. `build` and `check` share one code
//! path and differ only in what they do with the finished tree, which is what
//! makes the drift test meaningful instead of incidental.

use std::collections::BTreeMap;
use std::path::Path;

/// Every generated file, keyed by workspace-relative path with `/` separators.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GeneratedTree {
    /// Path to contents.
    files: BTreeMap<String, Vec<u8>>,
}

impl GeneratedTree {
    /// Records one generated file.
    pub fn insert(&mut self, path: &str, contents: impl Into<Vec<u8>>) {
        self.files.insert(path.to_owned(), contents.into());
    }

    /// Every generated path, sorted.
    #[must_use]
    pub fn file_names(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }

    /// The contents of one generated file.
    #[must_use]
    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Number of generated files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether nothing was generated, which is always a bug.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Writes the tree to disk, creating parent directories.
    ///
    /// # Errors
    ///
    /// Returns the first I/O failure, naming the file it happened on.
    pub fn write_to(&self, root: &Path) -> Result<(), String> {
        for (path, contents) in &self.files {
            let target = root.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create `{}`: {error}", parent.display()))?;
            }
            std::fs::write(&target, contents)
                .map_err(|error| format!("cannot write `{path}`: {error}"))?;
        }
        Ok(())
    }

    /// Compares the tree against what is on disk.
    ///
    /// Returns one line per drifting file: missing, unreadable, or different.
    #[must_use]
    pub fn diff_against_disk(&self, root: &Path) -> Vec<String> {
        let mut drift = Vec::new();
        for (path, expected) in &self.files {
            match std::fs::read(root.join(path)) {
                Err(error) => drift.push(format!("{path}: not on disk ({error})")),
                Ok(actual) if actual != *expected => {
                    drift.push(format!(
                        "{path}: differs ({} bytes on disk, {} generated)",
                        actual.len(),
                        expected.len()
                    ));
                }
                Ok(_) => {}
            }
        }
        drift
    }
}
