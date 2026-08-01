//! The filesystem executors.
//!
//! Every one is a pure function of an injected [`GuestFs`], so the whole argument,
//! bound, truncation and error corpus runs off-VM.
//!
//! The revision check is **stateless**: Brain passes the digest it read, the guest
//! hashes the file and compares. That deletes the old per-connection read cache,
//! which was scoped to one socket and therefore never protected two concurrent
//! agents. Two children editing the same file now get a real conflict — one wins,
//! the other receives `stale_write` with the current content window and re-reads.
//! An opaque `shell_exec` mutation advances nothing the guest tracks, so the next
//! structured edit simply sees a changed digest and conflicts, which is the honest
//! POSIX-race semantics.

use aex_hands_protocol::operation::{GuestPath, GuestRoot, Patch, PatchHunk};
use aex_wire::ids::ContentHash;

use crate::port::{EntryKind, FsError, GuestFs, digest};

/// Default number of lines a read returns when no window is named.
pub const DEFAULT_LINE_WINDOW: u32 = 200;

/// Largest number of lines a read may return.
pub const MAX_LINE_WINDOW: u32 = 10_000;

/// Largest number of bytes a read returns.
pub const MAX_READ_BYTES: usize = 400_000;

/// Largest number of entries a list returns.
pub const MAX_LIST_ENTRIES: usize = 2_000;

/// Largest number of matches a search returns by default.
pub const DEFAULT_SEARCH_MATCHES: u32 = 200;

/// Largest number of matches a search may return.
pub const MAX_SEARCH_MATCHES: u32 = 2_000;

/// Files larger than this are skipped by search.
pub const SEARCH_MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// Largest rendered match line, in characters.
pub const SEARCH_LINE_DISPLAY_CHARS: usize = 1_000;

/// Largest patch, in bytes.
pub const MAX_PATCH_BYTES: usize = 1_048_576;

/// How much of the current file a stale-write conflict reports back.
pub const STALE_WINDOW_BYTES: usize = 4_096;

/// A read window, in one-based inclusive lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    /// First line, one-based inclusive.
    pub start_line: u32,
    /// Last line, one-based inclusive.
    pub end_line: u32,
}

/// What a read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    /// The rendered text.
    pub text: String,
    /// The digest of the **whole** file, so a later edit can pin it.
    pub digest: ContentHash,
    /// The whole file's length in bytes.
    pub total_bytes: u64,
    /// The whole file's line count.
    pub total_lines: u32,
    /// Whether the returned text is a subset.
    pub truncated: bool,
    /// The human-readable notice appended to the text, when there is one.
    pub notice: Option<String>,
}

/// Reads a file, optionally windowed by line.
///
/// The digest is always over the whole file, never over the window: an edit pins
/// the file it read, and pinning a window would let a change outside the window
/// pass unnoticed.
///
/// # Errors
///
/// See [`FsError`]. A directory is [`FsError::IsADirectory`] rather than an empty
/// read.
pub fn read_file(
    fs: &dyn GuestFs,
    path: &GuestPath,
    range: Option<LineRange>,
) -> Result<ReadOutcome, FsError> {
    let meta = fs.lstat(path)?;
    if meta.kind == EntryKind::Directory {
        return Err(FsError::IsADirectory {
            path: path.as_str().to_owned(),
        });
    }
    let bytes = fs.read(path)?;
    let whole_digest = digest(&bytes);
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = u32::try_from(lines.len()).unwrap_or(u32::MAX);

    let window = range.unwrap_or(LineRange {
        start_line: 1,
        end_line: DEFAULT_LINE_WINDOW,
    });
    let start = window.start_line.max(1);
    let span = window.end_line.saturating_sub(start).saturating_add(1);
    let span = span.min(MAX_LINE_WINDOW);
    let end = start.saturating_add(span.saturating_sub(1));

    let from = usize::try_from(start.saturating_sub(1)).unwrap_or(usize::MAX);
    let to = usize::try_from(end).unwrap_or(usize::MAX).min(lines.len());
    let slice: Vec<&str> = if from >= lines.len() {
        Vec::new()
    } else {
        lines[from..to].to_vec()
    };
    let mut rendered = slice.join("\n");

    let mut notice = None;
    let mut truncated = to < lines.len() || from > 0;
    if truncated {
        notice = Some(format!(
            "[fs_read: lines {}-{} of {total_lines}]",
            start,
            to.max(from)
        ));
    }
    if rendered.len() > MAX_READ_BYTES {
        let cut = floor_char_boundary(&rendered, MAX_READ_BYTES);
        rendered.truncate(cut);
        truncated = true;
        notice = Some(format!("[truncated: first {cut} of {} bytes]", bytes.len()));
    }
    if let Some(text) = &notice {
        rendered.push('\n');
        rendered.push_str(text);
    }
    Ok(ReadOutcome {
        text: rendered,
        digest: whole_digest,
        total_bytes: meta.size,
        total_lines,
        truncated,
        notice,
    })
}

/// The largest index at or below `at` that is a UTF-8 character boundary.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut index = at.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// What a list produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOutcome {
    /// One `"<type> <size> <name>[/]"` line per entry.
    pub lines: Vec<String>,
    /// How many entries were visited, whether returned or not.
    pub visited: usize,
    /// Whether the listing was cut short.
    pub truncated: bool,
}

/// Lists a directory.
///
/// Symlinks are reported by `lstat` and **never followed**: following one would let
/// a link inside the workspace pull an arbitrary path in the rest of the filesystem
/// into a structured tool's answer. `.git` is listed but never descended, because
/// walking an object store produces tens of thousands of useless entries.
///
/// # Errors
///
/// See [`FsError`].
pub fn list_dir(
    fs: &dyn GuestFs,
    root: &GuestRoot,
    path: &GuestPath,
    recursive: bool,
    depth: u32,
    limit: usize,
) -> Result<ListOutcome, FsError> {
    let root_meta = fs.lstat(path)?;
    if root_meta.kind != EntryKind::Directory {
        return Err(FsError::NotADirectory {
            path: path.as_str().to_owned(),
        });
    }
    let cap = limit.min(MAX_LIST_ENTRIES);
    let mut lines = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    let mut frontier = vec![(path.clone(), 0u32)];

    while let Some((directory, level)) = frontier.pop() {
        for entry in fs.read_dir(&directory)? {
            visited += 1;
            if lines.len() >= cap {
                truncated = true;
                continue;
            }
            let suffix = if entry.meta.kind == EntryKind::Directory {
                "/"
            } else {
                ""
            };
            lines.push(format!(
                "{} {} {}{suffix}",
                entry.meta.kind.as_str(),
                entry.meta.size,
                entry.name
            ));
            let descend = recursive
                && entry.meta.kind == EntryKind::Directory
                && entry.name != ".git"
                && level + 1 < depth;
            if descend {
                let child = format!(
                    "{}/{}",
                    directory.as_str().trim_end_matches('/'),
                    entry.name
                );
                if let Ok(child) = GuestPath::parse(root, &child) {
                    frontier.push((child, level + 1));
                }
            }
        }
    }
    lines.sort();
    Ok(ListOutcome {
        lines,
        visited,
        truncated,
    })
}

/// Stats a path without following a symlink.
///
/// # Errors
///
/// See [`FsError`].
pub fn stat_path(fs: &dyn GuestFs, path: &GuestPath) -> Result<String, FsError> {
    let meta = fs.lstat(path)?;
    let mut line = format!(
        "{},{},{},{:o},{}",
        meta.kind.as_str(),
        meta.size,
        meta.mtime_ms,
        meta.mode,
        path.as_str()
    );
    if let Some(target) = &meta.target {
        line.push(',');
        line.push_str(target);
    }
    Ok(line)
}

/// Writes a file atomically.
///
/// # Errors
///
/// See [`FsError`].
pub fn write_file(
    fs: &dyn GuestFs,
    path: &GuestPath,
    bytes: &[u8],
    mode: u32,
) -> Result<u64, FsError> {
    fs.write_atomic(path, bytes, mode)
}

/// Which normalization made a replacement match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchMode {
    /// The expected text matched byte for byte.
    Exact,
    /// It matched after normalizing line endings.
    EolNormalized,
    /// It matched after trimming trailing whitespace on each line.
    TrailingWsNormalized,
    /// It matched after both normalizations.
    EolAndTrailingWsNormalized,
}

impl MatchMode {
    /// The tiers, in the order they are tried. The first tier that produces a
    /// *unique* match wins, so a looser normalization never silently overrides a
    /// stricter one that would also have matched.
    pub const TIERS: [Self; 4] = [
        Self::Exact,
        Self::EolNormalized,
        Self::TrailingWsNormalized,
        Self::EolAndTrailingWsNormalized,
    ];

    /// Applies this tier's normalization.
    #[must_use]
    pub fn normalize(self, text: &str) -> String {
        let eol = matches!(self, Self::EolNormalized | Self::EolAndTrailingWsNormalized);
        let trailing = matches!(
            self,
            Self::TrailingWsNormalized | Self::EolAndTrailingWsNormalized
        );
        let mut working = if eol {
            text.replace("\r\n", "\n")
        } else {
            text.to_owned()
        };
        if trailing {
            working = working
                .split('\n')
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n");
        }
        working
    }
}

/// Why an edit did not apply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The file changed since Brain read it.
    #[error("the file changed since it was read")]
    StaleWrite {
        /// The digest the file has now.
        current_digest: ContentHash,
        /// The length the file has now.
        current_len: u64,
        /// The first bytes of the current content, so the caller can re-read
        /// without a second round trip.
        current_window: String,
    },
    /// The expected text is not present.
    #[error("hunk {index}: the expected text is not present")]
    NotFound {
        /// Which hunk.
        index: usize,
    },
    /// The expected text is present more than once and the hunk asked for one.
    #[error("hunk {index}: the expected text occurs {count} times")]
    Ambiguous {
        /// Which hunk.
        index: usize,
        /// How many times it occurs.
        count: usize,
    },
    /// The patch is too large.
    #[error("the patch is {bytes} bytes, the ceiling is {MAX_PATCH_BYTES}")]
    PatchTooLarge {
        /// How large it was.
        bytes: usize,
    },
    /// The filesystem refused.
    #[error(transparent)]
    Fs(#[from] FsError),
}

/// What an edit produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    /// The digest of the file after the edit.
    pub digest: ContentHash,
    /// The tier each hunk matched under, in hunk order.
    pub applied: Vec<MatchMode>,
    /// The new length in bytes.
    pub bytes: u64,
}

/// Applies a revision-checked patch.
///
/// # Errors
///
/// See [`EditError`].
pub fn edit_file(
    fs: &dyn GuestFs,
    path: &GuestPath,
    expected: ContentHash,
    edit: &Patch,
) -> Result<EditOutcome, EditError> {
    let declared: usize = edit
        .hunks
        .iter()
        .map(|hunk| hunk.expected.len() + hunk.replacement.len())
        .sum();
    if declared > MAX_PATCH_BYTES {
        return Err(EditError::PatchTooLarge { bytes: declared });
    }

    let current = fs.read(path)?;
    let found = digest(&current);
    if found != expected {
        let text = String::from_utf8_lossy(&current);
        let cut = floor_char_boundary(&text, STALE_WINDOW_BYTES);
        return Err(EditError::StaleWrite {
            current_digest: found,
            current_len: current.len() as u64,
            current_window: text[..cut].to_owned(),
        });
    }

    let mut working = String::from_utf8_lossy(&current).into_owned();
    let mut applied = Vec::with_capacity(edit.hunks.len());
    for (index, hunk) in edit.hunks.iter().enumerate() {
        let (next, mode) = apply_hunk(&working, hunk, index)?;
        working = next;
        applied.push(mode);
    }

    let bytes = fs.write_atomic(path, working.as_bytes(), 0o644)?;
    Ok(EditOutcome {
        digest: digest(working.as_bytes()),
        applied,
        bytes,
    })
}

/// Applies one hunk, trying each normalization tier in order.
fn apply_hunk(
    text: &str,
    hunk: &PatchHunk,
    index: usize,
) -> Result<(String, MatchMode), EditError> {
    let mut ambiguity: Option<usize> = None;
    for tier in MatchMode::TIERS {
        let haystack = tier.normalize(text);
        let needle = tier.normalize(&hunk.expected);
        if needle.is_empty() {
            return Err(EditError::NotFound { index });
        }
        let count = haystack.matches(needle.as_str()).count();
        match count {
            0 => {}
            1 => {
                return Ok((
                    haystack.replacen(needle.as_str(), &hunk.replacement, 1),
                    tier,
                ));
            }
            many if hunk.occurrences == 0 => {
                // An explicit replace-all: every occurrence is intended, so the
                // multiplicity is not an ambiguity.
                let _ = many;
                return Ok((haystack.replace(needle.as_str(), &hunk.replacement), tier));
            }
            many if usize::try_from(hunk.occurrences).unwrap_or(usize::MAX) == many => {
                return Ok((haystack.replace(needle.as_str(), &hunk.replacement), tier));
            }
            many => {
                // Record it and keep going: a looser tier cannot resolve an
                // ambiguity a stricter one already has, but a stricter tier failing
                // to match at all must not hide a unique looser match.
                ambiguity.get_or_insert(many);
            }
        }
    }
    match ambiguity {
        Some(count) => Err(EditError::Ambiguous { index, count }),
        None => Err(EditError::NotFound { index }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LINE_WINDOW, EditError, LineRange, MAX_PATCH_BYTES, MatchMode, edit_file, list_dir,
        read_file, stat_path,
    };
    use crate::port::{DirEntry, EntryKind, FsError, GuestFs, Meta, digest};
    use aex_hands_protocol::operation::{GuestPath, GuestRoot, Patch, PatchHunk};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// An in-memory filesystem: the whole executor matrix runs off-VM.
    #[derive(Debug, Default)]
    struct MemoryFs {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        directories: Mutex<BTreeMap<String, Vec<DirEntry>>>,
        links: Mutex<BTreeMap<String, String>>,
        full: bool,
    }

    impl MemoryFs {
        fn with_file(path: &str, bytes: &[u8]) -> Self {
            let fs = Self::default();
            fs.files
                .lock()
                .expect("the lock is uncontended")
                .insert(path.to_owned(), bytes.to_vec());
            fs
        }

        fn content(&self, path: &str) -> Vec<u8> {
            self.files
                .lock()
                .expect("the lock is uncontended")
                .get(path)
                .cloned()
                .unwrap_or_default()
        }
    }

    impl GuestFs for MemoryFs {
        fn read(&self, path: &GuestPath) -> Result<Vec<u8>, FsError> {
            self.files
                .lock()
                .expect("the lock is uncontended")
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| FsError::NotFound {
                    path: path.as_str().to_owned(),
                })
        }

        fn write_atomic(&self, path: &GuestPath, bytes: &[u8], _mode: u32) -> Result<u64, FsError> {
            if self.full {
                return Err(FsError::DiskFull);
            }
            self.files
                .lock()
                .expect("the lock is uncontended")
                .insert(path.as_str().to_owned(), bytes.to_vec());
            Ok(bytes.len() as u64)
        }

        fn lstat(&self, path: &GuestPath) -> Result<Meta, FsError> {
            if let Some(target) = self
                .links
                .lock()
                .expect("the lock is uncontended")
                .get(path.as_str())
            {
                return Ok(Meta {
                    kind: EntryKind::Symlink,
                    size: 0,
                    mtime_ms: 1,
                    mode: 0o777,
                    target: Some(target.clone()),
                });
            }
            if self
                .directories
                .lock()
                .expect("the lock is uncontended")
                .contains_key(path.as_str())
            {
                return Ok(Meta {
                    kind: EntryKind::Directory,
                    size: 0,
                    mtime_ms: 1,
                    mode: 0o755,
                    target: None,
                });
            }
            let files = self.files.lock().expect("the lock is uncontended");
            let bytes = files.get(path.as_str()).ok_or_else(|| FsError::NotFound {
                path: path.as_str().to_owned(),
            })?;
            Ok(Meta {
                kind: EntryKind::File,
                size: bytes.len() as u64,
                mtime_ms: 1,
                mode: 0o644,
                target: None,
            })
        }

        fn read_dir(&self, path: &GuestPath) -> Result<Vec<DirEntry>, FsError> {
            self.directories
                .lock()
                .expect("the lock is uncontended")
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| FsError::NotFound {
                    path: path.as_str().to_owned(),
                })
        }
    }

    fn path(text: &str) -> GuestPath {
        GuestPath::parse(&GuestRoot::workspace(), text).expect("a contained path")
    }

    #[test]
    fn a_read_returns_the_whole_file_digest_even_for_a_window() {
        let body = (1..=500)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let fs = MemoryFs::with_file("/workspace/a.txt", body.as_bytes());
        let outcome = read_file(&fs, &path("/workspace/a.txt"), None).expect("the read succeeds");
        assert_eq!(outcome.digest, digest(body.as_bytes()));
        assert_eq!(outcome.total_lines, 500);
        assert!(outcome.truncated);
        assert!(
            outcome
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("of 500")),
            "the notice states how much was left out: {:?}",
            outcome.notice
        );
        assert!(outcome.text.starts_with("line 1\n"));
        assert!(
            outcome
                .text
                .contains(&format!("line {DEFAULT_LINE_WINDOW}")),
            "the default window is the first {DEFAULT_LINE_WINDOW} lines"
        );
    }

    #[test]
    fn a_read_window_is_honoured_and_clamped() {
        let body = (1..=50)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let fs = MemoryFs::with_file("/workspace/a.txt", body.as_bytes());
        let outcome = read_file(
            &fs,
            &path("/workspace/a.txt"),
            Some(LineRange {
                start_line: 10,
                end_line: 12,
            }),
        )
        .expect("the read succeeds");
        assert!(outcome.text.starts_with("line 10\nline 11\nline 12\n"));

        // A window past the end yields no lines rather than an error.
        let past = read_file(
            &fs,
            &path("/workspace/a.txt"),
            Some(LineRange {
                start_line: 999,
                end_line: 1_099,
            }),
        )
        .expect("the read succeeds");
        assert!(past.text.starts_with('\n') || past.text.starts_with('['));
    }

    #[test]
    fn a_directory_read_is_typed_rather_than_empty() {
        let fs = MemoryFs::default();
        fs.directories
            .lock()
            .expect("the lock is uncontended")
            .insert("/workspace".to_owned(), Vec::new());
        assert!(matches!(
            read_file(&fs, &path("/workspace"), None),
            Err(FsError::IsADirectory { .. })
        ));
    }

    #[test]
    fn stat_reports_a_symlink_without_following_it() {
        let fs = MemoryFs::default();
        fs.links
            .lock()
            .expect("the lock is uncontended")
            .insert("/workspace/link".to_owned(), "/etc/shadow".to_owned());
        let line = stat_path(&fs, &path("/workspace/link")).expect("the stat succeeds");
        assert!(line.starts_with("link,"));
        assert!(
            line.ends_with("/etc/shadow"),
            "the target is reported, not followed: {line}"
        );
    }

    #[test]
    fn a_listing_caps_entries_and_never_descends_into_git() {
        let fs = MemoryFs::default();
        let mut entries: Vec<DirEntry> = (0..10)
            .map(|n| DirEntry {
                name: format!("f{n}"),
                meta: Meta {
                    kind: EntryKind::File,
                    size: 1,
                    mtime_ms: 1,
                    mode: 0o644,
                    target: None,
                },
            })
            .collect();
        entries.push(DirEntry {
            name: ".git".to_owned(),
            meta: Meta {
                kind: EntryKind::Directory,
                size: 0,
                mtime_ms: 1,
                mode: 0o755,
                target: None,
            },
        });
        {
            let mut directories = fs.directories.lock().expect("the lock is uncontended");
            directories.insert("/workspace".to_owned(), entries);
            // A `.git` that would explode the listing if it were ever descended.
            directories.insert(
                "/workspace/.git".to_owned(),
                (0..10_000)
                    .map(|n| DirEntry {
                        name: format!("o{n}"),
                        meta: Meta {
                            kind: EntryKind::File,
                            size: 1,
                            mtime_ms: 1,
                            mode: 0o644,
                            target: None,
                        },
                    })
                    .collect(),
            );
        }

        let outcome = list_dir(
            &fs,
            &GuestRoot::workspace(),
            &path("/workspace"),
            true,
            5,
            2_000,
        )
        .expect("the list succeeds");
        assert_eq!(outcome.visited, 11, "`.git` is listed but never descended");
        assert!(outcome.lines.iter().any(|line| line.ends_with(".git/")));

        let capped = list_dir(
            &fs,
            &GuestRoot::workspace(),
            &path("/workspace"),
            false,
            1,
            3,
        )
        .expect("the list succeeds");
        assert_eq!(capped.lines.len(), 3);
        assert!(capped.truncated);
    }

    #[test]
    fn an_edit_pins_the_digest_and_a_concurrent_change_conflicts() {
        let original = b"hello world\n";
        let fs = MemoryFs::with_file("/workspace/a.txt", original);
        let pinned = digest(original);

        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "world".to_owned(),
                replacement: "there".to_owned(),
                occurrences: 1,
            }],
        };
        let outcome =
            edit_file(&fs, &path("/workspace/a.txt"), pinned, &patch).expect("the edit applies");
        assert_eq!(outcome.applied, vec![MatchMode::Exact]);
        assert_eq!(fs.content("/workspace/a.txt"), b"hello there\n");

        // The second agent still holds the original digest: it loses, and is told
        // exactly what the file is now.
        let Err(EditError::StaleWrite {
            current_digest,
            current_len,
            current_window,
        }) = edit_file(&fs, &path("/workspace/a.txt"), pinned, &patch)
        else {
            panic!("a stale write must conflict");
        };
        assert_eq!(current_digest, digest(b"hello there\n"));
        assert_eq!(current_len, 12);
        assert_eq!(current_window, "hello there\n");
    }

    #[test]
    fn an_opaque_shell_mutation_makes_the_next_structured_edit_conflict() {
        let fs = MemoryFs::with_file("/workspace/a.txt", b"one\n");
        let pinned = digest(b"one\n");
        // A `shell_exec` writes behind the guest's back. Nothing tracks it, and
        // nothing needs to: the digest simply no longer matches.
        fs.write_atomic(&path("/workspace/a.txt"), b"two\n", 0o644)
            .expect("the write succeeds");
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "one".to_owned(),
                replacement: "three".to_owned(),
                occurrences: 1,
            }],
        };
        assert!(matches!(
            edit_file(&fs, &path("/workspace/a.txt"), pinned, &patch),
            Err(EditError::StaleWrite { .. })
        ));
    }

    #[test]
    fn the_matching_tiers_are_tried_in_order_and_the_tier_used_is_reported() {
        // Only an end-of-line normalization can make this match.
        let body = "alpha\r\nbeta\r\n";
        let fs = MemoryFs::with_file("/workspace/a.txt", body.as_bytes());
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "alpha\nbeta".to_owned(),
                replacement: "gamma".to_owned(),
                occurrences: 1,
            }],
        };
        let outcome = edit_file(
            &fs,
            &path("/workspace/a.txt"),
            digest(body.as_bytes()),
            &patch,
        )
        .expect("the edit applies");
        assert_eq!(outcome.applied, vec![MatchMode::EolNormalized]);

        // And a trailing-whitespace-only difference reaches the third tier.
        let padded = "alpha   \nbeta\n";
        let fs = MemoryFs::with_file("/workspace/b.txt", padded.as_bytes());
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "alpha\nbeta".to_owned(),
                replacement: "gamma".to_owned(),
                occurrences: 1,
            }],
        };
        let outcome = edit_file(
            &fs,
            &path("/workspace/b.txt"),
            digest(padded.as_bytes()),
            &patch,
        )
        .expect("the edit applies");
        assert_eq!(outcome.applied, vec![MatchMode::TrailingWsNormalized]);
    }

    #[test]
    fn an_ambiguous_hunk_is_refused_and_an_explicit_replace_all_is_not() {
        let body = "x\nx\nx\n";
        let fs = MemoryFs::with_file("/workspace/a.txt", body.as_bytes());
        let single = Patch {
            hunks: vec![PatchHunk {
                expected: "x".to_owned(),
                replacement: "y".to_owned(),
                occurrences: 1,
            }],
        };
        assert!(matches!(
            edit_file(
                &fs,
                &path("/workspace/a.txt"),
                digest(body.as_bytes()),
                &single
            ),
            Err(EditError::Ambiguous { index: 0, count: 3 })
        ));

        let all = Patch {
            hunks: vec![PatchHunk {
                expected: "x".to_owned(),
                replacement: "y".to_owned(),
                occurrences: 0,
            }],
        };
        edit_file(
            &fs,
            &path("/workspace/a.txt"),
            digest(body.as_bytes()),
            &all,
        )
        .expect("an explicit replace-all is not ambiguous");
        assert_eq!(fs.content("/workspace/a.txt"), b"y\ny\ny\n");
    }

    #[test]
    fn a_hunk_that_matches_nothing_is_refused_and_nothing_is_written() {
        let body = "alpha\n";
        let fs = MemoryFs::with_file("/workspace/a.txt", body.as_bytes());
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "omega".to_owned(),
                replacement: "x".to_owned(),
                occurrences: 1,
            }],
        };
        assert!(matches!(
            edit_file(
                &fs,
                &path("/workspace/a.txt"),
                digest(body.as_bytes()),
                &patch
            ),
            Err(EditError::NotFound { index: 0 })
        ));
        assert_eq!(fs.content("/workspace/a.txt"), body.as_bytes());
    }

    #[test]
    fn an_oversized_patch_is_refused_before_the_file_is_even_read() {
        let fs = MemoryFs::default();
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "x".repeat(MAX_PATCH_BYTES + 1),
                replacement: String::new(),
                occurrences: 1,
            }],
        };
        // The file does not exist; an `FsError::NotFound` would prove the read
        // happened first.
        assert!(matches!(
            edit_file(&fs, &path("/workspace/missing.txt"), digest(b""), &patch),
            Err(EditError::PatchTooLarge { .. })
        ));
    }

    #[test]
    fn a_full_disk_is_reported_as_itself() {
        let mut fs = MemoryFs::with_file("/workspace/a.txt", b"one\n");
        fs.full = true;
        let patch = Patch {
            hunks: vec![PatchHunk {
                expected: "one".to_owned(),
                replacement: "two".to_owned(),
                occurrences: 1,
            }],
        };
        let outcome = edit_file(&fs, &path("/workspace/a.txt"), digest(b"one\n"), &patch);
        assert!(matches!(outcome, Err(EditError::Fs(FsError::DiskFull))));
        assert_eq!(FsError::DiskFull.code(), "disk_full");
    }
}
