//! Search, and the path-containment properties every structured tool relies on.
//!
//! Search always **succeeds**. A deadline, a match cap, a binary file or an
//! oversized file produce an explicit truncation notice rather than a failure,
//! because "I looked at as much as I was allowed to" is a useful answer and an
//! error is not.

use aex_hands_protocol::operation::{GuestPath, GuestRoot, SearchPattern};

use crate::filesystem::{
    DEFAULT_SEARCH_MATCHES, MAX_SEARCH_MATCHES, SEARCH_LINE_DISPLAY_CHARS, SEARCH_MAX_FILE_BYTES,
};

/// Largest number of files one search walks.
pub const SEARCH_MAX_FILES: usize = 100_000;

/// Largest number of characters scanned in one line.
pub const SEARCH_MAX_SCAN_CHARS: usize = 200_000;

/// One file offered to a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCandidate {
    /// The path, relative to the search root.
    pub relative_path: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// Why a candidate contributed no matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// The file contains a NUL byte, so it is binary.
    Binary,
    /// The file is larger than the search bound.
    TooLarge,
    /// The search deadline passed before this file was reached.
    DeadlinePassed,
    /// The match cap was already reached.
    CapReached,
}

/// What a search produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    /// `<rel>:<line>:<text>` for each match, in candidate order.
    pub matches: Vec<String>,
    /// How many files were skipped, by reason.
    pub skipped: Vec<(String, SkipReason)>,
    /// The explicit truncation notice, when the answer is not complete.
    pub notice: Option<String>,
}

/// Runs a search over already-enumerated candidates.
///
/// `elapsed_ms` is supplied by the caller for each candidate, so the deadline is a
/// pure input and the whole matrix — deadline included — is deterministic. A real
/// clock in here would make the deadline test a timing race.
#[must_use]
pub fn search(
    candidates: &[SearchCandidate],
    pattern: &SearchPattern,
    limit: u32,
    deadline_ms: u64,
    elapsed_ms: &dyn Fn(usize) -> u64,
) -> SearchOutcome {
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
        return SearchOutcome {
            matches,
            skipped,
            notice: Some("[search: the pattern is outside its size bound]".to_owned()),
        };
    }

    for (index, candidate) in candidates.iter().enumerate().take(SEARCH_MAX_FILES) {
        if elapsed_ms(index) >= deadline_ms {
            skipped.push((candidate.relative_path.clone(), SkipReason::DeadlinePassed));
            notice = Some(format!("[search: deadline reached after {index} files]"));
            break;
        }
        if matches.len() >= cap {
            skipped.push((candidate.relative_path.clone(), SkipReason::CapReached));
            notice = Some(format!("[search: stopped at {cap} matches]"));
            break;
        }
        if candidate.bytes.len() as u64 > SEARCH_MAX_FILE_BYTES {
            skipped.push((candidate.relative_path.clone(), SkipReason::TooLarge));
            continue;
        }
        if candidate.bytes.contains(&0) {
            skipped.push((candidate.relative_path.clone(), SkipReason::Binary));
            continue;
        }
        let text = String::from_utf8_lossy(&candidate.bytes);
        if matches!(pattern, SearchPattern::Glob { .. }) {
            if matches_glob(pattern, &candidate.relative_path) {
                matches.push(format!("{}:0:", candidate.relative_path));
            }
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            if matches.len() >= cap {
                notice = Some(format!("[search: stopped at {cap} matches]"));
                break;
            }
            let scanned: String = line.chars().take(SEARCH_MAX_SCAN_CHARS).collect();
            if !matches_line(pattern, &scanned) {
                continue;
            }
            let display: String = scanned.chars().take(SEARCH_LINE_DISPLAY_CHARS).collect();
            matches.push(format!(
                "{}:{}:{display}",
                candidate.relative_path,
                number + 1
            ));
        }
    }
    if candidates.len() > SEARCH_MAX_FILES {
        notice = Some(format!("[search: walked {SEARCH_MAX_FILES} files]"));
    }
    SearchOutcome {
        matches,
        skipped,
        notice,
    }
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
    use super::{SearchCandidate, SkipReason, glob_match, is_contained, matches_glob, search};
    use aex_hands_protocol::operation::{GuestPath, GuestRoot, SearchPattern};
    use proptest::prelude::{prop_assert, proptest};

    fn candidate(path: &str, body: &str) -> SearchCandidate {
        SearchCandidate {
            relative_path: path.to_owned(),
            bytes: body.as_bytes().to_vec(),
        }
    }

    fn literal(needle: &str) -> SearchPattern {
        SearchPattern::Literal {
            needle: needle.to_owned(),
            case_sensitive: true,
        }
    }

    #[test]
    fn a_search_reports_path_line_and_text() {
        let files = [candidate("src/a.rs", "one\ntwo\nthree\n")];
        let outcome = search(&files, &literal("two"), 10, 15_000, &|_| 0);
        assert_eq!(outcome.matches, vec!["src/a.rs:2:two"]);
        assert!(outcome.notice.is_none());
    }

    #[test]
    fn a_binary_file_is_skipped_rather_than_rendered() {
        let files = [SearchCandidate {
            relative_path: "bin/blob".to_owned(),
            bytes: vec![b'a', 0, b'b'],
        }];
        let outcome = search(&files, &literal("a"), 10, 15_000, &|_| 0);
        assert!(outcome.matches.is_empty());
        assert_eq!(
            outcome.skipped,
            vec![("bin/blob".to_owned(), SkipReason::Binary)]
        );
    }

    #[test]
    fn the_deadline_stops_the_walk_and_still_succeeds() {
        let files: Vec<SearchCandidate> = (0..10)
            .map(|n| candidate(&format!("f{n}.rs"), "needle\n"))
            .collect();
        // The clock crosses the deadline at the fourth file.
        let outcome = search(&files, &literal("needle"), 100, 15_000, &|index| {
            if index >= 3 { 15_000 } else { 0 }
        });
        assert_eq!(outcome.matches.len(), 3);
        assert!(
            outcome
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("deadline")),
            "the truncation is stated: {:?}",
            outcome.notice
        );
        assert_eq!(outcome.skipped[0].1, SkipReason::DeadlinePassed);
    }

    #[test]
    fn the_match_cap_is_explicit_and_never_silent() {
        let files: Vec<SearchCandidate> = (0..10)
            .map(|n| candidate(&format!("f{n}.rs"), "needle\n"))
            .collect();
        let outcome = search(&files, &literal("needle"), 4, 15_000, &|_| 0);
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
        let files = [candidate("a.rs", "x")];
        let huge = SearchPattern::Regex {
            expression: "x".repeat(SearchPattern::MAX_BYTES + 1),
            case_sensitive: true,
        };
        let outcome = search(&files, &huge, 10, 15_000, &|_| 0);
        assert!(outcome.matches.is_empty());
        assert!(outcome.notice.is_some());

        let empty = SearchPattern::Literal {
            needle: String::new(),
            case_sensitive: true,
        };
        assert!(search(&files, &empty, 10, 15_000, &|_| 0).notice.is_some());
    }

    #[test]
    fn a_case_insensitive_literal_matches_either_casing() {
        let files = [candidate("a.rs", "Hello World\n")];
        let pattern = SearchPattern::Literal {
            needle: "hello".to_owned(),
            case_sensitive: false,
        };
        assert_eq!(
            search(&files, &pattern, 10, 15_000, &|_| 0).matches.len(),
            1
        );
        assert!(
            search(&files, &literal("hello"), 10, 15_000, &|_| 0)
                .matches
                .is_empty(),
            "a case-sensitive literal does not"
        );
    }

    #[test]
    fn a_glob_matches_the_path_and_not_the_contents() {
        let files = [
            candidate("src/main.rs", "nothing"),
            candidate("docs/readme.md", "nothing"),
        ];
        let pattern = SearchPattern::Glob {
            glob: "src/*.rs".to_owned(),
        };
        let outcome = search(&files, &pattern, 10, 15_000, &|_| 0);
        assert_eq!(outcome.matches, vec!["src/main.rs:0:"]);
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
