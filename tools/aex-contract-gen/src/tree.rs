//! The in-memory output tree.
//!
//! Every emitter writes here, never to disk. `build` and `check` share one code
//! path and differ only in what they do with the finished tree, which is what
//! makes the drift test meaningful instead of incidental.

use std::collections::{BTreeMap, BTreeSet};
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

    /// The one authored file permitted to sit inside a generated directory.
    const AUTHORED_COMPANION: &'static str = "README.md";

    /// Writes the tree to disk, creating parent directories and removing any
    /// generated file the tree no longer produces.
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
        for path in self.stale_files(root) {
            std::fs::remove_file(root.join(&path))
                .map_err(|error| format!("cannot remove stale `{path}`: {error}"))?;
        }
        Ok(())
    }

    /// Every file sitting in a generated directory that this tree does not
    /// produce.
    ///
    /// A directory that holds a generated file is owned by the generator, so an
    /// unrecognized file in it is a leftover from an input that has since been
    /// deleted — the one drift class a file-by-file comparison cannot see.
    #[must_use]
    pub fn stale_files(&self, root: &Path) -> Vec<String> {
        let mut directories: BTreeSet<&str> = BTreeSet::new();
        for path in self.files.keys() {
            if let Some((directory, _)) = path.rsplit_once('/') {
                directories.insert(directory);
            }
        }
        let mut stale = Vec::new();
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(root.join(directory)) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                if name == Self::AUTHORED_COMPANION {
                    continue;
                }
                let path = format!("{directory}/{name}");
                if !self.files.contains_key(&path) {
                    stale.push(path);
                }
            }
        }
        stale.sort();
        stale
    }

    /// Compares the tree against what is on disk.
    ///
    /// Returns one line per drifting file: missing, unreadable, or different.
    #[must_use]
    pub fn diff_against_disk(&self, root: &Path) -> Vec<String> {
        let mut drift: Vec<String> = self
            .stale_files(root)
            .into_iter()
            .map(|path| format!("{path}: on disk but no longer generated"))
            .collect();
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
