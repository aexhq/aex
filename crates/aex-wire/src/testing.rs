//! The conformance-corpus loader.
//!
//! The tree is resolved from `CARGO_MANIFEST_DIR` at compile time. There is no
//! environment variable and no runtime discovery, because a corpus that can be
//! "not found" is a corpus that silently stops running. An empty load panics
//! with the path it tried, so a missing tree is as loud as a failing case.

use std::path::{Path, PathBuf};

/// The conformance tree.
pub mod corpus {
    use super::{Path, PathBuf};

    /// The absolute path of `conformance/`.
    #[must_use]
    pub fn root() -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../conformance")).to_path_buf()
    }

    /// Reads one corpus file as raw text.
    ///
    /// # Panics
    ///
    /// Panics when the file is absent or empty, naming the path it tried.
    #[must_use]
    pub fn read(relative: &str) -> String {
        let path = root().join(relative);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("cannot read corpus file `{}`: {error}", path.display())
        });
        assert!(
            !text.trim().is_empty(),
            "corpus file `{}` is empty; an empty case set is a silent skip",
            path.display()
        );
        text
    }

    /// Reads one JSON Lines corpus file, ignoring blank lines and `#` comments.
    ///
    /// # Panics
    ///
    /// Panics when the file is absent, empty, or contains a line that is not
    /// valid JSON for `T`.
    #[must_use]
    pub fn read_jsonl<T: serde::de::DeserializeOwned>(relative: &str) -> Vec<T> {
        let text = read(relative);
        let mut cases = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let case = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!(
                    "{relative}:{}: not valid JSON for the case type: {error}",
                    index + 1
                )
            });
            cases.push(case);
        }
        assert!(
            !cases.is_empty(),
            "corpus file `{relative}` contained no cases"
        );
        cases
    }
}
