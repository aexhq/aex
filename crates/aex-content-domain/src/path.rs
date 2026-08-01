//! Workspace paths and the one ordering rule.
//!
//! A [`NormalizedPath`] is a relative POSIX path that is already in its normal
//! form. `parse` **rejects** rather than repairs, so parsing is idempotent by
//! construction: nothing that survives it would change under a second pass, and
//! two different spellings of the same location can never both be stored.
//!
//! Comparison is UTF-8 byte order, everywhere, for every path comparison
//! (D-23). No locale, no case folding, no Unicode collation. That rule is what
//! makes a Merkle root portable across languages.

use core::cmp::Ordering;
use core::fmt;

/// Longest accepted path, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;

/// Longest accepted single segment, in bytes.
pub const MAX_SEGMENT_BYTES: usize = 255;

/// A validated, already-normalized relative POSIX path.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NormalizedPath(String);

impl NormalizedPath {
    /// Parses a relative POSIX path.
    ///
    /// Rejects the empty string, a leading or trailing `/`, an empty segment
    /// (`//`), a backslash, a NUL or any other C0/C1 control character, a `.`
    /// or `..` segment, an over-long path or segment, and any name that a
    /// second `parse` would not reproduce byte for byte.
    ///
    /// # Errors
    ///
    /// Returns the [`PathError`] naming the first violated rule.
    pub fn parse(text: &str) -> Result<Self, PathError> {
        if text.is_empty() {
            return Err(PathError::Empty);
        }
        if text.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong { bytes: text.len() });
        }
        if text.starts_with('/') {
            return Err(PathError::Absolute);
        }
        if text.ends_with('/') {
            return Err(PathError::TrailingSeparator);
        }
        if let Some(position) = text.find('\\') {
            return Err(PathError::Backslash { position });
        }
        if let Some(position) = text
            .char_indices()
            .find(|(_, character)| is_control(*character))
            .map(|(position, _)| position)
        {
            return Err(PathError::ControlCharacter { position });
        }
        for segment in text.split('/') {
            if segment.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return Err(PathError::RelativeSegment);
            }
            if segment.len() > MAX_SEGMENT_BYTES {
                return Err(PathError::SegmentTooLong {
                    bytes: segment.len(),
                });
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrows the path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path bytes, which are also its canonical encoding.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Byte length of the path.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the path is empty. Never true for a parsed value; present so
    /// `len` does not stand alone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The path segments, in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The strict ancestors of this path, shortest first.
    #[must_use]
    pub fn ancestors(&self) -> Vec<Self> {
        let mut out = Vec::new();
        let mut prefix = String::new();
        let mut segments = self.0.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                break;
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            out.push(Self(prefix.clone()));
        }
        out
    }

    /// The total order used for every path comparison: UTF-8 byte order.
    #[must_use]
    pub fn cmp_bytes(&self, other: &Self) -> Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl PartialOrd for NormalizedPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NormalizedPath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_bytes(other)
    }
}

impl fmt::Display for NormalizedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for NormalizedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NormalizedPath({})", self.0)
    }
}

const fn is_control(character: char) -> bool {
    let code = character as u32;
    code < 0x20 || (0x7f..=0x9f).contains(&code)
}

/// A symlink target, stored **resolved** and root-relative.
///
/// The raw target may be written with `..` segments; it is resolved against the
/// link's own parent directory at parse time and stored in its root-relative
/// form. A target that would leave the workspace root is rejected here rather
/// than at write time, so an escaping link is unrepresentable and the canonical
/// tree encoding has exactly one spelling per target.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelativeInternalPath(NormalizedPath);

impl RelativeInternalPath {
    /// Resolves a symlink target relative to the directory holding `link`.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] when the target is absolute, malformed, or
    /// escapes the root.
    pub fn parse(link: &NormalizedPath, target: &str) -> Result<Self, PathError> {
        if target.is_empty() {
            return Err(PathError::Empty);
        }
        if target.starts_with('/') {
            return Err(PathError::Absolute);
        }
        if target.ends_with('/') {
            return Err(PathError::TrailingSeparator);
        }
        if let Some(position) = target.find('\\') {
            return Err(PathError::Backslash { position });
        }
        if let Some(position) = target
            .char_indices()
            .find(|(_, character)| is_control(*character))
            .map(|(position, _)| position)
        {
            return Err(PathError::ControlCharacter { position });
        }
        let mut resolved: Vec<&str> = link
            .0
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, parent)| parent.to_vec())
            .unwrap_or_default();
        for segment in target.split('/') {
            match segment {
                "" => return Err(PathError::EmptySegment),
                "." => return Err(PathError::RelativeSegment),
                ".." => {
                    if resolved.pop().is_none() {
                        return Err(PathError::EscapesRoot);
                    }
                }
                other => {
                    if other.len() > MAX_SEGMENT_BYTES {
                        return Err(PathError::SegmentTooLong { bytes: other.len() });
                    }
                    resolved.push(other);
                }
            }
        }
        if resolved.is_empty() {
            return Err(PathError::EscapesRoot);
        }
        NormalizedPath::parse(&resolved.join("/")).map(Self)
    }

    /// The resolved, root-relative target text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The resolved target as a normalized path.
    #[must_use]
    pub fn path(&self) -> &NormalizedPath {
        &self.0
    }
}

impl fmt::Display for RelativeInternalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Debug for RelativeInternalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelativeInternalPath({})", self.0)
    }
}

/// A POSIX file mode, rendered as four octal digits with a leading zero.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileMode(u16);

impl FileMode {
    /// The default mode for a regular file.
    pub const FILE: Self = Self(0o0644);
    /// The default mode for a directory.
    pub const DIRECTORY: Self = Self(0o0755);
    /// The only mode a symlink may carry.
    pub const SYMLINK: Self = Self(0o0777);

    /// Wraps a permission word.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::ModeOutOfRange`] above `0o7777`.
    pub const fn new(bits: u16) -> Result<Self, PathError> {
        if bits > 0o7777 {
            return Err(PathError::ModeOutOfRange { bits });
        }
        Ok(Self(bits))
    }

    /// The raw permission word.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl fmt::Display for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04o}", self.0)
    }
}

impl fmt::Debug for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileMode({self})")
    }
}

/// Why a path, symlink target or mode was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path was the empty string.
    #[error("path must not be empty")]
    Empty,
    /// The path began with `/`.
    #[error("path must be relative")]
    Absolute,
    /// The path ended with `/`.
    #[error("path must not end with a separator")]
    TrailingSeparator,
    /// Two separators appeared in a row.
    #[error("path must not contain an empty segment")]
    EmptySegment,
    /// A `.` or `..` segment appeared.
    #[error("path must not contain a `.` or `..` segment")]
    RelativeSegment,
    /// A backslash appeared.
    #[error("path must not contain a backslash (byte {position})")]
    Backslash {
        /// Byte offset of the backslash.
        position: usize,
    },
    /// A C0 or C1 control character appeared.
    #[error("path must not contain a control character (byte {position})")]
    ControlCharacter {
        /// Byte offset of the control character.
        position: usize,
    },
    /// The path exceeded [`MAX_PATH_BYTES`].
    #[error("path must be at most {MAX_PATH_BYTES} bytes, found {bytes}")]
    TooLong {
        /// Observed byte length.
        bytes: usize,
    },
    /// A segment exceeded [`MAX_SEGMENT_BYTES`].
    #[error("path segment must be at most {MAX_SEGMENT_BYTES} bytes, found {bytes}")]
    SegmentTooLong {
        /// Observed segment byte length.
        bytes: usize,
    },
    /// A symlink target resolved outside the workspace root.
    #[error("symlink target must stay inside the workspace root")]
    EscapesRoot,
    /// A mode word exceeded `0o7777`.
    #[error("file mode {bits:#o} is above 0o7777")]
    ModeOutOfRange {
        /// The offending permission word.
        bits: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::{FileMode, NormalizedPath, PathError, RelativeInternalPath};

    #[test]
    fn accepted_paths_are_idempotent_under_parse() {
        for text in ["a", "a/b", "src/lib.rs", ".hidden", "a b/c.txt", "ä/ß"] {
            let parsed = NormalizedPath::parse(text).expect("accepted");
            assert_eq!(parsed.as_str(), text);
            assert_eq!(NormalizedPath::parse(parsed.as_str()), Ok(parsed));
        }
    }

    #[test]
    fn the_rejected_class_is_never_accepted() {
        assert_eq!(NormalizedPath::parse(""), Err(PathError::Empty));
        assert_eq!(NormalizedPath::parse("/a"), Err(PathError::Absolute));
        assert_eq!(NormalizedPath::parse("a/"), Err(PathError::TrailingSeparator));
        assert_eq!(NormalizedPath::parse("a//b"), Err(PathError::EmptySegment));
        assert_eq!(NormalizedPath::parse("a/./b"), Err(PathError::RelativeSegment));
        assert_eq!(NormalizedPath::parse("a/../b"), Err(PathError::RelativeSegment));
        assert_eq!(NormalizedPath::parse(".."), Err(PathError::RelativeSegment));
        assert_eq!(
            NormalizedPath::parse("a\\b"),
            Err(PathError::Backslash { position: 1 })
        );
        assert_eq!(
            NormalizedPath::parse("a\0b"),
            Err(PathError::ControlCharacter { position: 1 })
        );
        assert_eq!(
            NormalizedPath::parse("a\u{85}b"),
            Err(PathError::ControlCharacter { position: 1 })
        );
    }

    #[test]
    fn comparison_is_utf8_byte_order() {
        let a = NormalizedPath::parse("a").expect("accepted");
        let big_z = NormalizedPath::parse("Z").expect("accepted");
        let accented = NormalizedPath::parse("é").expect("accepted");
        assert!(big_z < a);
        assert!(a < accented);
    }

    #[test]
    fn ancestors_are_the_strict_prefixes() {
        let path = NormalizedPath::parse("a/b/c.txt").expect("accepted");
        let ancestors: Vec<String> = path
            .ancestors()
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect();
        assert_eq!(ancestors, vec!["a".to_owned(), "a/b".to_owned()]);
        assert!(NormalizedPath::parse("a").expect("accepted").ancestors().is_empty());
    }

    #[test]
    fn symlink_targets_must_stay_inside_the_root() {
        let link = NormalizedPath::parse("a/b/link").expect("accepted");
        assert_eq!(
            RelativeInternalPath::parse(&link, "c").expect("accepted").as_str(),
            "a/b/c"
        );
        assert_eq!(
            RelativeInternalPath::parse(&link, "../c").expect("accepted").as_str(),
            "a/c"
        );
        assert_eq!(
            RelativeInternalPath::parse(&link, "../../c"),
            Err(PathError::EscapesRoot)
        );
        assert_eq!(
            RelativeInternalPath::parse(&link, "/etc/passwd"),
            Err(PathError::Absolute)
        );
    }

    #[test]
    fn file_mode_renders_four_octal_digits() {
        assert_eq!(FileMode::FILE.to_string(), "0644");
        assert_eq!(FileMode::new(0o0755).expect("in range").to_string(), "0755");
        assert_eq!(
            FileMode::new(0o10000),
            Err(PathError::ModeOutOfRange { bits: 0o10000 })
        );
    }
}
