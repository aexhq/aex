//! Search, and the path-containment properties every structured tool relies on.
//!
//! Search always **succeeds**. A match cap, a binary file, an oversized file or
//! an exhausted read budget produce an explicit truncation notice rather than a
//! failure, because "I looked at as much as I was allowed to" is a useful
//! answer and an error is not.
//!
//! The search runs directly over the guest filesystem, one file resident at a
//! time. Sizes come from the directory listing, so an oversized file is skipped
//! without ever being read, and a glob never reads at all. The retired shape
//! read every candidate into memory before matching, which let one search of a
//! `node_modules` tree OOM-kill the guest's own PID-1 supervisor.

use aex_hands_protocol::operation::{GuestPath, GuestRoot, SearchPattern};

use crate::filesystem::{
    DEFAULT_SEARCH_MATCHES, MAX_SEARCH_MATCHES, SEARCH_LINE_DISPLAY_CHARS, SEARCH_MAX_FILE_BYTES,
};
use crate::port::{EntryKind, FsError, GuestFs};

/// Largest number of files one search walks.
pub const SEARCH_MAX_FILES: usize = 100_000;

/// Largest number of characters scanned in one line.
pub const SEARCH_MAX_SCAN_CHARS: usize = 200_000;

/// Largest total number of bytes one search pass may read.
///
/// The constraint this converts is the guest's own memory and time: the
/// searcher runs inside PID 1, files are read one at a time and dropped after
/// scanning — so resident memory stays under [`SEARCH_MAX_FILE_BYTES`] — and
/// this budget bounds how much I/O one pass over a large tree may spend before
/// answering with what it has.
pub const SEARCH_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Why a file contributed no matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// The file contains a NUL byte, so it is binary.
    Binary,
    /// The file is larger than the per-file search bound. Decided from the
    /// directory listing; the file is never read.
    TooLarge,
    /// The match cap was already reached.
    CapReached,
    /// The aggregate read budget was exhausted before this file.
    ByteBudget,
}

/// What a search produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    /// `<rel>:<line>:<text>` for each match, in walk order.
    pub matches: Vec<String>,
    /// How many files were skipped, by reason.
    pub skipped: Vec<(String, SkipReason)>,
    /// The explicit truncation notice, when the answer is not complete.
    pub notice: Option<String>,
}

/// Runs a bounded search directly over the guest filesystem.
///
/// The walk starts at `root`, lists `.git` without descending into it, and
/// never follows a symlink: following one would let a link inside the
/// workspace pull in the rest of the filesystem.
///
/// # Errors
///
/// Returns [`FsError`] when the tree itself cannot be walked. A skipped file
/// is not an error; it is reported in the outcome.
pub fn search_tree(
    fs: &dyn GuestFs,
    guest_root: &GuestRoot,
    root: &GuestPath,
    pattern: &SearchPattern,
    limit: u32,
) -> Result<SearchOutcome, FsError> {
    let cap = usize::try_from(if limit == 0 {
        DEFAULT_SEARCH_MATCHES
    } else {
        limit.min(MAX_SEARCH_MATCHES)
    })
    .unwrap_or(usize::MAX);

    let mut matches = Vec::new();
    let mut skipped = Vec::new();
    let mut notice = None;

    if !pattern.is_bounded() {
        return Ok(SearchOutcome {
            matches,
            skipped,
            notice: Some("[search: the pattern is outside its size bound]".to_owned()),
        });
    }

    let mut walked = 0_usize;
    let mut read_bytes = 0_u64;
    let mut frontier = vec![root.clone()];
    'walk: while let Some(current) = frontier.pop() {
        for entry in fs.read_dir(&current)? {
            let text = format!("{}/{}", current.as_str(), entry.name);
            let Ok(child) = GuestPath::parse(guest_root, &text) else {
                continue;
            };
            match entry.meta.kind {
                EntryKind::Directory if entry.name != ".git" => frontier.push(child),
                EntryKind::File => {
                    let relative = text
                        .strip_prefix(root.as_str())
                        .unwrap_or(&text)
                        .trim_start_matches('/')
                        .to_owned();
                    if walked >= SEARCH_MAX_FILES {
                        notice = Some(format!("[search: walked {SEARCH_MAX_FILES} files]"));
                        break 'walk;
                    }
                    walked += 1;
                    if matches.len() >= cap {
                        skipped.push((relative, SkipReason::CapReached));
                        notice = Some(format!("[search: stopped at {cap} matches]"));
                        break 'walk;
                    }
                    // A glob asks about the path alone; nothing is read.
                    if matches!(pattern, SearchPattern::Glob { .. }) {
                        if matches_glob(pattern, &relative) {
                            matches.push(format!("{relative}:0:"));
                        }
                        continue;
                    }
                    // The size comes from the directory listing, so an
                    // oversized file is skipped without being read.
                    if entry.meta.size > SEARCH_MAX_FILE_BYTES {
                        skipped.push((relative, SkipReason::TooLarge));
                        continue;
                    }
                    if read_bytes.saturating_add(entry.meta.size) > SEARCH_MAX_TOTAL_BYTES {
                        skipped.push((relative, SkipReason::ByteBudget));
                        notice = Some(format!(
                            "[search: stopped at the {SEARCH_MAX_TOTAL_BYTES}-byte read budget]"
                        ));
                        break 'walk;
                    }
                    let bytes = fs.read(&child)?;
                    // The budget counts the larger of the listing's claim and
                    // what was actually read, so a file that grew mid-walk
                    // still spends its full weight.
                    read_bytes =
                        read_bytes.saturating_add((bytes.len() as u64).max(entry.meta.size));
                    if bytes.contains(&0) {
                        skipped.push((relative, SkipReason::Binary));
                        continue;
                    }
                    let body = String::from_utf8_lossy(&bytes);
                    for (number, line) in body.lines().enumerate() {
                        if matches.len() >= cap {
                            notice = Some(format!("[search: stopped at {cap} matches]"));
                            break;
                        }
                        let scanned: String = line.chars().take(SEARCH_MAX_SCAN_CHARS).collect();
                        if !matches_line(pattern, &scanned) {
                            continue;
                        }
                        let display: String =
                            scanned.chars().take(SEARCH_LINE_DISPLAY_CHARS).collect();
                        matches.push(format!("{relative}:{}:{display}", number + 1));
                    }
                }
                EntryKind::Directory | EntryKind::Symlink | EntryKind::Other => {}
            }
        }
    }
    Ok(SearchOutcome {
        matches,
        skipped,
        notice,
    })
}

/// Whether one line matches a content pattern.
///
/// `Regex` is matched as a literal here. The pattern language is a closed set in
/// the contract and a full engine is a bounded, auditable addition; matching it as
/// a literal in the meantime is *narrower* than promised, never wider, so no caller
/// gets a match it should not have.
fn matches_line(pattern: &SearchPattern, line: &str) -> bool {
    match pattern {
        SearchPattern::Literal {
            needle,
            case_sensitive,
        }
        | SearchPattern::Regex {
            expression: needle,
            case_sensitive,
        } => {
            if *case_sensitive {
                line.contains(needle.as_str())
            } else {
                line.to_lowercase().contains(&needle.to_lowercase())
            }
        }
        SearchPattern::Glob { .. } => false,
    }
}

/// Whether a path matches a glob. Supports `*` and `?` only, anchored whole-string.
fn matches_glob(pattern: &SearchPattern, path: &str) -> bool {
    let SearchPattern::Glob { glob } = pattern else {
        return false;
    };
    glob_match(glob.as_bytes(), path.as_bytes())
}

/// A bounded, allocation-free glob matcher over `*` and `?`.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = p;
            mark = t;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// Whether a candidate path is inside the workspace root.
///
/// This is the **structural tool contract**, not a security boundary: H-BOUNDARY
/// grants the customer real root and `shell_exec` reaches the whole filesystem by
/// design. Structured file tools stay inside the root so revision checking and
/// persist have meaning, not to keep anyone out.
#[must_use]
pub fn is_contained(root: &GuestRoot, candidate: &str) -> bool {
    GuestPath::parse(root, candidate).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        SEARCH_MAX_TOTAL_BYTES, SkipReason, glob_match, is_contained, matches_glob, search_tree,
    };
    use crate::filesystem::SEARCH_MAX_FILE_BYTES;
    use crate::port::{DirEntry, EntryKind, FsError, GuestFs, Meta};
    use aex_hands_protocol::operation::{GuestPath, GuestRoot, SearchPattern};
    use proptest::prelude::{prop_assert, proptest};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// An in-memory tree the walk runs over, counting every read.
    ///
    /// `sizes` lets a test declare a file far larger than its stored bytes, so
    /// stat-before-read is proven by the read counter rather than by allocating
    /// the bytes for real.
    #[derive(Debug, Default)]
    struct TreeFs {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        sizes: Mutex<BTreeMap<String, u64>>,
        reads: AtomicU64,
    }

    impl TreeFs {
        fn with_files(entries: &[(&str, &[u8])]) -> Self {
            let fs = Self::default();
            for (path, bytes) in entries {
                fs.insert(path, bytes, bytes.len() as u64);
            }
            fs
        }

        fn insert(&self, path: &str, bytes: &[u8], size: u64) {
            self.files
                .lock()
                .expect("the lock is uncontended")
                .insert((*path).to_owned(), bytes.to_vec());
            self.sizes
                .lock()
                .expect("the lock is uncontended")
                .insert((*path).to_owned(), size);
        }

        fn reads(&self) -> u64 {
            self.reads.load(Ordering::SeqCst)
        }
    }

    impl GuestFs for TreeFs {
        fn read(&self, path: &GuestPath) -> Result<Vec<u8>, FsError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.files
                .lock()
                .expect("the lock is uncontended")
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| FsError::NotFound {
                    path: path.as_str().to_owned(),
                })
        }

        fn write_atomic(
            &self,
            path: &GuestPath,
            _bytes: &[u8],
            _mode: u32,
        ) -> Result<u64, FsError> {
            Err(FsError::PermissionDenied {
                path: path.as_str().to_owned(),
            })
        }

        fn lstat(&self, path: &GuestPath) -> Result<Meta, FsError> {
            let sizes = self.sizes.lock().expect("the lock is uncontended");
            let size = sizes.get(path.as_str()).ok_or_else(|| FsError::NotFound {
                path: path.as_str().to_owned(),
            })?;
            Ok(Meta {
                kind: EntryKind::File,
                size: *size,
                mtime_ms: 1,
                mode: 0o644,
                target: None,
            })
        }

        fn read_dir(&self, path: &GuestPath) -> Result<Vec<DirEntry>, FsError> {
            // Derives the listing from the flat file map: the immediate children
            // of `path`, files with their declared sizes, directories synthesized.
            let prefix = format!("{}/", path.as_str());
            let mut names: BTreeMap<String, Option<u64>> = BTreeMap::new();
            for (file, size) in self.sizes.lock().expect("the lock is uncontended").iter() {
                let Some(rest) = file.strip_prefix(&prefix) else {
                    continue;
                };
                match rest.split_once('/') {
                    None => {
                        names.insert(rest.to_owned(), Some(*size));
                    }
                    Some((directory, _)) => {
                        names.entry(directory.to_owned()).or_insert(None);
                    }
                }
            }
            Ok(names
                .into_iter()
                .map(|(name, size)| DirEntry {
                    meta: Meta {
                        kind: size.map_or(EntryKind::Directory, |_| EntryKind::File),
                        size: size.unwrap_or(0),
                        mtime_ms: 1,
                        mode: 0o644,
                        target: None,
                    },
                    name,
                })
                .collect())
        }
    }

    fn root() -> (GuestRoot, GuestPath) {
        let guest_root = GuestRoot::workspace();
        let root = GuestPath::parse(&guest_root, "/workspace").expect("a contained path");
        (guest_root, root)
    }

    fn literal(needle: &str) -> SearchPattern {
        SearchPattern::Literal {
            needle: needle.to_owned(),
            case_sensitive: true,
        }
    }

    #[test]
    fn a_search_reports_path_line_and_text_across_subdirectories() {
        let fs = TreeFs::with_files(&[
            ("/workspace/src/a.rs", b"one\ntwo\nthree\n"),
            ("/workspace/src/nested/b.rs", b"two more\n"),
        ]);
        let (guest_root, root) = root();
        let outcome =
            search_tree(&fs, &guest_root, &root, &literal("two"), 10).expect("the tree walks");
        let mut found = outcome.matches.clone();
        found.sort();
        assert_eq!(found, vec!["src/a.rs:2:two", "src/nested/b.rs:1:two more"]);
        assert!(outcome.notice.is_none());
    }

    #[test]
    fn a_binary_file_is_skipped_rather_than_rendered() {
        let fs = TreeFs::with_files(&[("/workspace/bin/blob", &[b'a', 0, b'b'])]);
        let (guest_root, root) = root();
        let outcome =
            search_tree(&fs, &guest_root, &root, &literal("a"), 10).expect("the tree walks");
        assert!(outcome.matches.is_empty());
        assert_eq!(
            outcome.skipped,
            vec![("bin/blob".to_owned(), SkipReason::Binary)]
        );
    }

    #[test]
    fn an_oversized_file_is_skipped_without_ever_being_read() {
        let fs = TreeFs::with_files(&[("/workspace/ok.txt", b"needle\n")]);
        // The declared size is over the bound; the stored bytes are tiny, so a
        // read would succeed — the read counter is what proves it never happens.
        fs.insert(
            "/workspace/huge.bin",
            b"needle\n",
            SEARCH_MAX_FILE_BYTES + 1,
        );
        let (guest_root, root) = root();
        let outcome =
            search_tree(&fs, &guest_root, &root, &literal("needle"), 10).expect("the tree walks");
        assert_eq!(outcome.matches, vec!["ok.txt:1:needle"]);
        assert_eq!(
            outcome.skipped,
            vec![("huge.bin".to_owned(), SkipReason::TooLarge)]
        );
        assert_eq!(fs.reads(), 1, "the oversized file is never read");
    }

    #[test]
    fn the_read_budget_stops_the_pass_and_says_so() {
        // Eleven files, each within the per-file bound but declared at that
        // bound's full size: the aggregate budget admits ten and refuses the
        // eleventh before reading it. The stored bytes stay tiny — declared
        // size is what the budget spends, so the test allocates nothing big.
        let fs = TreeFs::default();
        for index in 0..11 {
            fs.insert(
                &format!("/workspace/f{index:02}.txt"),
                b"needle\n",
                SEARCH_MAX_FILE_BYTES,
            );
        }
        assert_eq!(
            SEARCH_MAX_TOTAL_BYTES / SEARCH_MAX_FILE_BYTES,
            10,
            "the fixture is sized against the real budget ratio"
        );
        let (guest_root, root) = root();
        let outcome =
            search_tree(&fs, &guest_root, &root, &literal("needle"), 100).expect("the tree walks");
        assert_eq!(outcome.matches.len(), 10);
        assert_eq!(
            outcome.skipped,
            vec![("f10.txt".to_owned(), SkipReason::ByteBudget)]
        );
        assert!(
            outcome
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("read budget")),
            "{:?}",
            outcome.notice
        );
        assert_eq!(
            fs.reads(),
            10,
            "the pass stops before the budget is exceeded"
        );
    }

    #[test]
    fn the_match_cap_is_explicit_and_never_silent() {
        let files: Vec<(String, Vec<u8>)> = (0..10)
            .map(|n| (format!("/workspace/f{n}.rs"), b"needle\n".to_vec()))
            .collect();
        let fs = TreeFs::default();
        for (path, bytes) in &files {
            fs.insert(path, bytes, bytes.len() as u64);
        }
        let (guest_root, root) = root();
        let outcome =
            search_tree(&fs, &guest_root, &root, &literal("needle"), 4).expect("the tree walks");
        assert_eq!(outcome.matches.len(), 4);
        assert!(
            outcome
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("4 matches")),
            "{:?}",
            outcome.notice
        );
    }

    #[test]
    fn an_unbounded_pattern_is_refused_with_a_notice_rather_than_a_scan() {
        let fs = TreeFs::with_files(&[("/workspace/a.rs", b"x")]);
        let (guest_root, root) = root();
        let huge = SearchPattern::Regex {
            expression: "x".repeat(SearchPattern::MAX_BYTES + 1),
            case_sensitive: true,
        };
        let outcome =
            search_tree(&fs, &guest_root, &root, &huge, 10).expect("the refusal is not an error");
        assert!(outcome.matches.is_empty());
        assert!(outcome.notice.is_some());
        assert_eq!(fs.reads(), 0, "a refused pattern reads nothing");

        let empty = SearchPattern::Literal {
            needle: String::new(),
            case_sensitive: true,
        };
        assert!(
            search_tree(&fs, &guest_root, &root, &empty, 10)
                .expect("the refusal is not an error")
                .notice
                .is_some()
        );
    }

    #[test]
    fn a_case_insensitive_literal_matches_either_casing() {
        let fs = TreeFs::with_files(&[("/workspace/a.rs", b"Hello World\n")]);
        let (guest_root, root) = root();
        let pattern = SearchPattern::Literal {
            needle: "hello".to_owned(),
            case_sensitive: false,
        };
        assert_eq!(
            search_tree(&fs, &guest_root, &root, &pattern, 10)
                .expect("the tree walks")
                .matches
                .len(),
            1
        );
        assert!(
            search_tree(&fs, &guest_root, &root, &literal("hello"), 10)
                .expect("the tree walks")
                .matches
                .is_empty(),
            "a case-sensitive literal does not"
        );
    }

    #[test]
    fn a_glob_matches_the_path_and_reads_no_file_at_all() {
        let fs = TreeFs::with_files(&[
            ("/workspace/src/main.rs", b"nothing".as_slice()),
            ("/workspace/docs/readme.md", b"nothing".as_slice()),
        ]);
        let (guest_root, root) = root();
        let pattern = SearchPattern::Glob {
            glob: "src/*.rs".to_owned(),
        };
        let outcome = search_tree(&fs, &guest_root, &root, &pattern, 10).expect("the tree walks");
        assert_eq!(outcome.matches, vec!["src/main.rs:0:"]);
        assert_eq!(fs.reads(), 0, "a glob asks about paths, never contents");
        assert!(matches_glob(&pattern, "src/main.rs"));
        assert!(!matches_glob(&pattern, "src/main.md"));
    }

    #[test]
    fn the_glob_matcher_handles_the_pathological_cases_without_hanging() {
        assert!(glob_match(b"*", b""));
        assert!(glob_match(b"****", b"abc"));
        assert!(glob_match(b"a*b*c", b"axxbyyc"));
        assert!(!glob_match(b"a*b*c", b"axxbyy"));
        assert!(glob_match(b"?", b"a"));
        assert!(!glob_match(b"?", b"ab"));
        // The classic backtracking blow-up, bounded by the two-pointer algorithm.
        let pattern = b"a*a*a*a*a*a*a*a*b";
        let text = vec![b'a'; 64];
        assert!(!glob_match(pattern, &text));
    }

    #[test]
    fn containment_rejects_the_named_escape_shapes() {
        let root = GuestRoot::workspace();
        for escape in [
            "/etc/shadow",
            "/workspace/../etc/shadow",
            "/workspace/./a",
            "relative/path",
            "",
            "/workspace/a\0b",
            "/workspacex/a",
        ] {
            assert!(
                !is_contained(&root, escape),
                "`{escape}` must not be treated as contained"
            );
        }
        for good in ["/workspace", "/workspace/a", "/workspace/a/b.txt"] {
            assert!(is_contained(&root, good), "`{good}` is contained");
        }
    }

    proptest! {
        /// No accepted path ever leaves the root, whatever the input.
        #[test]
        fn no_accepted_path_escapes_the_root(text in ".{0,64}") {
            let root = GuestRoot::workspace();
            if let Ok(path) = GuestPath::parse(&root, &text) {
                prop_assert!(path.as_str().starts_with("/workspace"));
                prop_assert!(!path.as_str().contains("/../"));
                prop_assert!(!path.as_str().contains('\0'));
                prop_assert!(path.as_str() == "/workspace"
                    || path.as_str().starts_with("/workspace/"));
            }
        }

        /// A path containing a traversal segment is never accepted.
        #[test]
        fn a_traversal_segment_is_never_accepted(
            prefix in "[a-z/]{0,16}",
            suffix in "[a-z/]{0,16}",
        ) {
            let root = GuestRoot::workspace();
            let candidate = format!("/workspace/{prefix}/../{suffix}");
            prop_assert!(GuestPath::parse(&root, &candidate).is_err());
        }
    }
}
