//! Selectors and the persist mirror.
//!
//! The selector grammar is `*`, `?` and whole-segment `**`. There is no
//! negation, no character class and no `[`, `]` or `!`, so a selector cannot
//! express "everything except" and the exclusion list is the only way to remove
//! something. Exclusions apply after inclusion and win.
//!
//! Mirror semantics: a selected live path is created or updated; a selected
//! durable path absent from live is deleted; unselected and excluded durable
//! paths are byte-identical afterwards. An unsupported selected node is a hard
//! error raised **before** any root is proposed.

use std::collections::{BTreeMap, BTreeSet};

use aex_content_domain::{
    ContentDigest, ContentRoot, EntryNode, NodeKind, NormalizedPath, TreeError, TreeView,
};
use aex_wire::ids::{OperationId, SessionId};
use aex_wire::types::Timestamp;

/// One segment of a selector pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorSegment {
    /// A whole-segment `**`: matches any number of segments, including none.
    AnyDepth,
    /// A pattern over exactly one segment.
    Pattern(Vec<PatternAtom>),
}

/// One atom inside a single-segment pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternAtom {
    /// Exactly this character.
    Literal(char),
    /// Any one character.
    AnyOne,
    /// Any run of characters, including none, inside one segment.
    AnyRun,
}

/// A validated selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    segments: Vec<SelectorSegment>,
    source: String,
}

impl Selector {
    /// Parses a selector.
    ///
    /// # Errors
    ///
    /// Returns [`SelectorError`] for an empty pattern, a `**` that is not a whole
    /// segment, or any character-class or negation syntax.
    pub fn parse(text: &str) -> Result<Self, SelectorError> {
        if text.is_empty() {
            return Err(SelectorError::Empty);
        }
        if let Some(position) = text
            .char_indices()
            .find(|(_, value)| matches!(value, '[' | ']' | '!' | '{' | '}'))
            .map(|(position, _)| position)
        {
            return Err(SelectorError::UnsupportedSyntax { position });
        }
        if text.starts_with('/') || text.ends_with('/') || text.contains("//") {
            return Err(SelectorError::Malformed);
        }

        let mut segments = Vec::new();
        for raw in text.split('/') {
            if raw == "**" {
                segments.push(SelectorSegment::AnyDepth);
                continue;
            }
            if raw.contains("**") {
                return Err(SelectorError::PartialAnyDepth);
            }
            if raw.is_empty() {
                return Err(SelectorError::Malformed);
            }
            let atoms = raw
                .chars()
                .map(|value| match value {
                    '*' => PatternAtom::AnyRun,
                    '?' => PatternAtom::AnyOne,
                    other => PatternAtom::Literal(other),
                })
                .collect();
            segments.push(SelectorSegment::Pattern(atoms));
        }
        Ok(Self {
            segments,
            source: text.to_owned(),
        })
    }

    /// The text the selector was parsed from. Round-trips exactly.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether the selector matches a path.
    #[must_use]
    pub fn matches(&self, path: &NormalizedPath) -> bool {
        let segments: Vec<&str> = path.segments().collect();
        match_segments(&self.segments, &segments)
    }
}

fn match_segments(pattern: &[SelectorSegment], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((SelectorSegment::AnyDepth, rest)) => {
            (0..=path.len()).any(|skip| match_segments(rest, &path[skip..]))
        }
        Some((SelectorSegment::Pattern(atoms), rest)) => match path.split_first() {
            None => false,
            Some((head, tail)) => {
                match_atoms(atoms, &head.chars().collect::<Vec<char>>())
                    && match_segments(rest, tail)
            }
        },
    }
}

fn match_atoms(atoms: &[PatternAtom], text: &[char]) -> bool {
    match atoms.split_first() {
        None => text.is_empty(),
        Some((PatternAtom::AnyRun, rest)) => {
            (0..=text.len()).any(|skip| match_atoms(rest, &text[skip..]))
        }
        Some((PatternAtom::AnyOne, rest)) => !text.is_empty() && match_atoms(rest, &text[1..]),
        Some((PatternAtom::Literal(expected), rest)) => match text.split_first() {
            Some((head, tail)) if head == expected => match_atoms(rest, tail),
            _ => false,
        },
    }
}

/// Why a selector was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    /// The pattern was empty.
    #[error("selector must not be empty")]
    Empty,
    /// A character-class or negation character appeared.
    #[error("selector character {position} is unsupported syntax")]
    UnsupportedSyntax {
        /// Where.
        position: usize,
    },
    /// `**` appeared inside a segment rather than as a whole one.
    #[error("`**` must be a whole segment")]
    PartialAnyDepth,
    /// The pattern had a leading, trailing or doubled separator.
    #[error("selector is malformed")]
    Malformed,
    /// An explicitly empty include list was supplied.
    #[error("an explicitly empty include list selects nothing and is rejected")]
    EmptyInclude,
}

/// What a persist selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistSelection {
    include: Option<Vec<Selector>>,
    exclude: Vec<Selector>,
}

impl PersistSelection {
    /// Parses a selection.
    ///
    /// An omitted `include` selects the whole workspace; an explicitly **empty**
    /// `include` is rejected here, before any live access, because it can only
    /// mean the caller built the list wrongly.
    ///
    /// # Errors
    ///
    /// Returns [`SelectorError`] for an empty include list or any invalid
    /// pattern.
    pub fn parse(
        include: Option<Vec<String>>,
        exclude: Vec<String>,
    ) -> Result<Self, SelectorError> {
        let include = match include {
            None => None,
            Some(patterns) if patterns.is_empty() => return Err(SelectorError::EmptyInclude),
            Some(patterns) => Some(dedupe_sorted(patterns)?),
        };
        Ok(Self {
            include,
            exclude: dedupe_sorted(exclude)?,
        })
    }

    /// Whether a path is selected. Include first, then exclude; exclude wins.
    #[must_use]
    pub fn selects(&self, path: &NormalizedPath) -> bool {
        let included = self
            .include
            .as_ref()
            .is_none_or(|patterns| patterns.iter().any(|selector| selector.matches(path)));
        included && !self.exclude.iter().any(|selector| selector.matches(path))
    }

    /// The digest of the selection, deduped and sorted before hashing so two
    /// callers that ask for the same thing produce the same fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> ContentDigest {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(b"aex.persist.selection.v1");
        match &self.include {
            None => buffer.push(0),
            Some(patterns) => {
                buffer.push(1);
                push_patterns(&mut buffer, patterns);
            }
        }
        push_patterns(&mut buffer, &self.exclude);
        ContentDigest::of(&buffer)
    }
}

fn push_patterns(buffer: &mut Vec<u8>, patterns: &[Selector]) {
    let count = u32::try_from(patterns.len())
        .unwrap_or_else(|_| unreachable!("a selection is bounded well below u32::MAX"));
    buffer.extend_from_slice(&count.to_le_bytes());
    for selector in patterns {
        let bytes = selector.as_str().as_bytes();
        let length = u32::try_from(bytes.len())
            .unwrap_or_else(|_| unreachable!("a selector is bounded well below u32::MAX"));
        buffer.extend_from_slice(&length.to_le_bytes());
        buffer.extend_from_slice(bytes);
    }
}

fn dedupe_sorted(patterns: Vec<String>) -> Result<Vec<Selector>, SelectorError> {
    let mut unique: BTreeSet<String> = BTreeSet::new();
    for pattern in patterns {
        Selector::parse(&pattern)?;
        unique.insert(pattern);
    }
    unique
        .into_iter()
        .map(|text| Selector::parse(&text))
        .collect()
}

/// How much a persist would move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistShape {
    /// How many leaves change.
    pub changed_leaves: u64,
    /// How many body bytes move.
    pub bytes_moved: u64,
}

/// The plan a persist produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistPlan {
    /// The root the persist would write.
    pub next_root: ContentRoot,
    /// Whether anything changes at all.
    pub changed: bool,
    /// Paths that appear.
    pub added: Vec<NormalizedPath>,
    /// Paths whose node changes, including metadata-only changes.
    pub updated: Vec<NormalizedPath>,
    /// Paths that disappear.
    pub deleted: Vec<NormalizedPath>,
    /// Body bytes that actually move.
    pub bytes_moved: u64,
    /// The shape, for the inline/continued classification.
    pub shape: PersistShape,
}

/// Why a persist could not be planned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistError {
    /// A selected live node is not a supported kind.
    #[error("selected path `{path}` is an unsupported node")]
    UnsupportedNode {
        /// Which path.
        path: NormalizedPath,
        /// What kind it is.
        kind: NodeKind,
    },
    /// The rebuilt tree was rejected.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// A receipt was replayed against a different session or selection.
    #[error("receipt does not match the replaying command")]
    ReceiptMismatch,
}

/// Plans a mirror of the selected live paths onto the durable tree.
///
/// # Errors
///
/// Returns [`PersistError`] when a selected live node is unsupported or the
/// rebuilt tree is rejected.
pub fn plan_persist(
    durable: &TreeView,
    live: &TreeView,
    selection: &PersistSelection,
) -> Result<PersistPlan, PersistError> {
    let mut next: BTreeMap<NormalizedPath, EntryNode> = durable.entries().clone();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut deleted = Vec::new();
    let mut bytes_moved = 0_u64;

    for (path, node) in live.entries() {
        if !selection.selects(path) {
            continue;
        }
        match node.kind() {
            NodeKind::File | NodeKind::Directory | NodeKind::Symlink => {}
        }
        match durable.get(path) {
            None => {
                added.push(path.clone());
                bytes_moved = bytes_moved.saturating_add(node.logical_bytes());
                next.insert(path.clone(), node.clone());
            }
            Some(existing) if existing == node => {}
            Some(existing) => {
                updated.push(path.clone());
                // Bytes move only when a body digest actually changes; a
                // metadata-only update moves nothing.
                if existing.body() != node.body() {
                    bytes_moved = bytes_moved.saturating_add(node.logical_bytes());
                }
                next.insert(path.clone(), node.clone());
            }
        }
    }

    for path in durable.entries().keys() {
        if selection.selects(path) && live.get(path).is_none() {
            deleted.push(path.clone());
            next.remove(path);
        }
    }

    let entries: Vec<aex_content_domain::TreeEntry> = next
        .into_iter()
        .map(|(path, node)| aex_content_domain::TreeEntry { path, node })
        .collect();
    let rebuilt = TreeView::build(durable.workspace(), &entries)?;
    let changed = !added.is_empty() || !updated.is_empty() || !deleted.is_empty();

    Ok(PersistPlan {
        next_root: if changed {
            rebuilt.root()
        } else {
            durable.root()
        },
        changed,
        shape: PersistShape {
            changed_leaves: (added.len() + updated.len() + deleted.len()) as u64,
            bytes_moved,
        },
        added,
        updated,
        deleted,
        bytes_moved,
    })
}

/// What a committed persist recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistReceipt {
    /// The operation that wrote it.
    pub operation: OperationId,
    /// The session it belongs to.
    pub session: SessionId,
    /// The root it left behind.
    pub root: ContentRoot,
    /// Whether anything changed.
    pub changed: bool,
    /// The persist revision after it.
    pub persist_revision: u64,
    /// How many paths appeared.
    pub added: u64,
    /// How many changed.
    pub updated: u64,
    /// How many disappeared.
    pub deleted: u64,
    /// How many body bytes moved.
    pub bytes_moved: u64,
    /// When it committed.
    pub last_persisted_at: Timestamp,
    /// The fingerprint of the selection it used.
    pub selector_fingerprint: ContentDigest,
}

/// Accepts a replayed receipt, or refuses it.
///
/// On a conflict the receipt is re-read and accepted **only** when its session
/// and selector fingerprint match; anything else is a different command wearing
/// the same operation id.
///
/// # Errors
///
/// Returns [`PersistError::ReceiptMismatch`] when either does not match.
pub fn replay_receipt<'a>(
    receipt: &'a PersistReceipt,
    session: SessionId,
    fingerprint: &ContentDigest,
) -> Result<&'a PersistReceipt, PersistError> {
    if receipt.session != session || receipt.selector_fingerprint != *fingerprint {
        return Err(PersistError::ReceiptMismatch);
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use aex_content_domain::NormalizedPath;

    use super::{PersistSelection, Selector, SelectorError};

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::parse(text).expect("valid")
    }

    #[test]
    fn the_grammar_is_exactly_star_question_and_whole_segment_doublestar() {
        assert!(Selector::parse("src/*.rs").is_ok());
        assert!(Selector::parse("**/lib.rs").is_ok());
        assert!(Selector::parse("a?c").is_ok());
        assert_eq!(Selector::parse(""), Err(SelectorError::Empty));
        assert_eq!(
            Selector::parse("src/**.rs"),
            Err(SelectorError::PartialAnyDepth)
        );
        assert!(matches!(
            Selector::parse("src/[ab].rs"),
            Err(SelectorError::UnsupportedSyntax { .. })
        ));
        assert!(matches!(
            Selector::parse("!src"),
            Err(SelectorError::UnsupportedSyntax { .. })
        ));
    }

    #[test]
    fn matching_is_segment_wise_and_dotfiles_participate() {
        let star = Selector::parse("src/*").expect("valid");
        assert!(star.matches(&path("src/lib.rs")));
        assert!(star.matches(&path("src/.hidden")));
        assert!(!star.matches(&path("src/a/b.rs")));

        let deep = Selector::parse("src/**").expect("valid");
        assert!(deep.matches(&path("src/a/b.rs")));
        assert!(deep.matches(&path("src")));

        let exact = Selector::parse("src/lib.rs").expect("valid");
        assert!(exact.matches(&path("src/lib.rs")));
        assert!(!exact.matches(&path("src/lib.rs.bak")));
    }

    #[test]
    fn an_explicitly_empty_include_is_rejected_before_any_access() {
        assert_eq!(
            PersistSelection::parse(Some(Vec::new()), Vec::new()),
            Err(SelectorError::EmptyInclude)
        );
        let whole = PersistSelection::parse(None, Vec::new()).expect("valid");
        assert!(whole.selects(&path("anything/at/all")));
    }

    #[test]
    fn exclusion_wins_and_the_fingerprint_is_order_free() {
        let selection = PersistSelection::parse(
            Some(vec!["src/**".to_owned()]),
            vec!["src/generated/**".to_owned()],
        )
        .expect("valid");
        assert!(selection.selects(&path("src/lib.rs")));
        assert!(!selection.selects(&path("src/generated/api.rs")));

        let reordered = PersistSelection::parse(
            Some(vec!["src/**".to_owned(), "src/**".to_owned()]),
            vec!["src/generated/**".to_owned()],
        )
        .expect("valid");
        assert_eq!(selection.fingerprint(), reordered.fingerprint());
    }
}
