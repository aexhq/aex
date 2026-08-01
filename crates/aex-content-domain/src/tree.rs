//! The Merkle tree: canonical pages, content-defined boundaries, roots and
//! copy-on-write mutation.
//!
//! Two rules make a root portable and history independent (D-06, D-23):
//!
//! * leaves are sorted and unique by **UTF-8 byte order**, everywhere;
//! * page boundaries are **content defined** — a page closes after the entry
//!   whose path hashes below [`SPLIT_THRESHOLD`], or when one more entry would
//!   push the page past [`crate::placement::TREE_PAGE_TARGET_BYTES`].
//!
//! Both rules depend only on the sorted prefix, so the same entry set always
//! produces the same root whatever order it was written in. A fixed-fanout
//! B-tree does not have that property: a front insertion reshapes every page
//! after it.
//!
//! The root binds the workspace id, so physical page reuse can never cross an
//! encryption domain.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::ids::{PrefixedId as _, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::descriptor::MediaType;
use crate::digest::{ContentDigest, PageDigest};
use crate::path::{FileMode, NormalizedPath, RelativeInternalPath};
use crate::pin::{Pin, RootKind};
use crate::placement::TREE_PAGE_TARGET_BYTES;

/// Domain tag for a leaf page.
const LEAF_TAG: &[u8] = b"aex.tree.leaf.v1";
/// Domain tag for a branch page.
const BRANCH_TAG: &[u8] = b"aex.tree.branch.v1";
/// Domain tag for the root binding.
const ROOT_TAG: &[u8] = b"aex.tree.root.v1";
/// Domain tag for the boundary function.
const SPLIT_TAG: &[u8] = b"aex.tree.split.v1";

/// A path is a page boundary when its salted hash falls below this value,
/// giving a mean fanout of 64.
pub const SPLIT_THRESHOLD: u16 = u16::MAX / 64;

/// Deepest tree the level-salted boundary function supports.
///
/// Each level consumes two bytes of one 32-byte BLAKE3 digest. Sixteen levels
/// at a mean fanout of 64 addresses more entries than any workspace can hold, so
/// exceeding it is a corrupted input rather than a capacity limit.
const MAX_LEVELS: usize = 16;

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// Which kind of filesystem node an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// An internal relative symlink.
    Symlink,
}

impl NodeKind {
    /// The stable discriminant used in every canonical byte encoding.
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        match self {
            Self::File => 1,
            Self::Directory => 2,
            Self::Symlink => 3,
        }
    }
}

/// One supported filesystem node.
///
/// UID, GID, ACLs, xattrs, devices, sockets, FIFOs and hard-link identity are
/// unrepresentable rather than dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryNode {
    /// A regular file.
    File {
        /// SHA-256 of the file body.
        body: ContentDigest,
        /// Plaintext byte length.
        size_bytes: u64,
        /// POSIX permission bits.
        mode: FileMode,
        /// Modification time.
        mtime: Timestamp,
        /// Declared media type, when one is known.
        media_type: Option<MediaType>,
    },
    /// A directory. Present only when it was explicitly recorded; trees are
    /// legitimately sparse.
    Directory {
        /// POSIX permission bits.
        mode: FileMode,
        /// Modification time.
        mtime: Timestamp,
    },
    /// An internal relative symlink.
    Symlink {
        /// The resolved, root-relative target.
        target: RelativeInternalPath,
        /// POSIX permission bits.
        mode: FileMode,
        /// Modification time.
        mtime: Timestamp,
    },
}

impl EntryNode {
    /// Which kind of node this is.
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        match self {
            Self::File { .. } => NodeKind::File,
            Self::Directory { .. } => NodeKind::Directory,
            Self::Symlink { .. } => NodeKind::Symlink,
        }
    }

    /// The logical bytes this node contributes. Directories and symlinks
    /// contribute zero.
    #[must_use]
    pub const fn logical_bytes(&self) -> u64 {
        match self {
            Self::File { size_bytes, .. } => *size_bytes,
            Self::Directory { .. } | Self::Symlink { .. } => 0,
        }
    }

    /// The body digest, when the node has one.
    #[must_use]
    pub const fn body(&self) -> Option<&ContentDigest> {
        match self {
            Self::File { body, .. } => Some(body),
            Self::Directory { .. } | Self::Symlink { .. } => None,
        }
    }
}

/// One `(path, node)` leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Where the node lives.
    pub path: NormalizedPath,
    /// What the node is.
    pub node: EntryNode,
}

/// A page of leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafPage {
    /// The leaves, sorted and unique by UTF-8 byte order.
    pub entries: Vec<TreeEntry>,
}

/// One child reference inside a branch page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChild {
    /// The lowest path in the child subtree.
    pub separator: NormalizedPath,
    /// The child page.
    pub child: PageDigest,
    /// How many leaves the subtree holds.
    pub subtree_entries: u64,
    /// How many logical bytes the subtree holds.
    pub subtree_bytes: u64,
}

/// A page of child references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPage {
    /// The children, ordered by separator.
    pub children: Vec<BranchChild>,
}

/// One Merkle page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    /// A page of leaves.
    Leaf(LeafPage),
    /// A page of child references.
    Branch(BranchPage),
}

impl TreeNode {
    /// How many leaves the page's subtree holds.
    #[must_use]
    pub fn subtree_entries(&self) -> u64 {
        match self {
            Self::Leaf(page) => page.entries.len() as u64,
            Self::Branch(page) => page
                .children
                .iter()
                .map(|child| child.subtree_entries)
                .sum(),
        }
    }

    /// How many logical bytes the page's subtree holds.
    #[must_use]
    pub fn subtree_bytes(&self) -> u64 {
        match self {
            Self::Leaf(page) => page
                .entries
                .iter()
                .map(|entry| entry.node.logical_bytes())
                .sum(),
            Self::Branch(page) => page.children.iter().map(|child| child.subtree_bytes).sum(),
        }
    }

    /// The lowest path in the page's subtree, when the page is not empty.
    #[must_use]
    pub fn separator(&self) -> Option<&NormalizedPath> {
        match self {
            Self::Leaf(page) => page.entries.first().map(|entry| &entry.path),
            Self::Branch(page) => page.children.first().map(|child| &child.separator),
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical bytes
// ---------------------------------------------------------------------------

/// A canonical `u32` count.
///
/// Every count this crate encodes is bounded far below `u32::MAX` by the page
/// target and the path length limit, so a failure here is a corrupted value
/// rather than a reachable input.
fn canonical_u32(value: usize) -> [u8; 4] {
    u32::try_from(value)
        .unwrap_or_else(|_| unreachable!("canonical counts are bounded by the page target"))
        .to_le_bytes()
}

fn push_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&canonical_u32(bytes.len()));
    out.extend_from_slice(bytes);
}

fn push_node(out: &mut Vec<u8>, node: &EntryNode) {
    out.push(node.kind().discriminant());
    match node {
        EntryNode::File {
            body,
            size_bytes,
            mode,
            mtime,
            media_type,
        } => {
            out.extend_from_slice(body.as_bytes());
            out.extend_from_slice(&size_bytes.to_le_bytes());
            out.extend_from_slice(&mode.bits().to_le_bytes());
            out.extend_from_slice(&mtime.unix_millis().to_le_bytes());
            match media_type {
                Some(value) => {
                    out.push(1);
                    push_len_prefixed(out, value.as_str().as_bytes());
                }
                None => out.push(0),
            }
        }
        EntryNode::Directory { mode, mtime } => {
            out.extend_from_slice(&mode.bits().to_le_bytes());
            out.extend_from_slice(&mtime.unix_millis().to_le_bytes());
        }
        EntryNode::Symlink {
            target,
            mode,
            mtime,
        } => {
            push_len_prefixed(out, target.as_str().as_bytes());
            out.extend_from_slice(&mode.bits().to_le_bytes());
            out.extend_from_slice(&mtime.unix_millis().to_le_bytes());
        }
    }
}

/// The canonical bytes of one page.
///
/// Hand-written, fixed-width little-endian and length-prefixed. Never `serde`
/// (D-25): a serialization bump must not be able to move a persisted root.
///
/// * `leaf   = b"aex.tree.leaf.v1"   || u32 count || entries`
/// * `branch = b"aex.tree.branch.v1" || u32 count || children`
#[must_use]
pub fn canonical_page_bytes(node: &TreeNode) -> Vec<u8> {
    let mut out = Vec::new();
    match node {
        TreeNode::Leaf(page) => {
            out.extend_from_slice(LEAF_TAG);
            out.extend_from_slice(&canonical_u32(page.entries.len()));
            for entry in &page.entries {
                push_len_prefixed(&mut out, entry.path.as_bytes());
                push_node(&mut out, &entry.node);
            }
        }
        TreeNode::Branch(page) => {
            out.extend_from_slice(BRANCH_TAG);
            out.extend_from_slice(&canonical_u32(page.children.len()));
            for child in &page.children {
                push_len_prefixed(&mut out, child.separator.as_bytes());
                out.extend_from_slice(child.child.as_bytes());
                out.extend_from_slice(&child.subtree_entries.to_le_bytes());
                out.extend_from_slice(&child.subtree_bytes.to_le_bytes());
            }
        }
    }
    out
}

/// The digest of one page.
#[must_use]
pub fn page_digest(node: &TreeNode) -> PageDigest {
    PageDigest::of(&canonical_page_bytes(node))
}

/// Whether a path closes a leaf page.
///
/// `blake3(b"aex.tree.split.v1" || path)[0..2] < SPLIT_THRESHOLD`, read as a
/// little-endian `u16`.
#[must_use]
pub fn is_boundary(path: &NormalizedPath) -> bool {
    boundary_at_level(path, 0)
}

/// The level-salted boundary function.
///
/// Level 0 is the leaf level. Each level reads its own two bytes of the same
/// salted digest, because reusing one predicate at every level would make every
/// branch child a boundary and the tree would never converge.
fn boundary_at_level(path: &NormalizedPath, level: usize) -> bool {
    debug_assert!(level < MAX_LEVELS);
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPLIT_TAG);
    hasher.update(path.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let offset = level * 2;
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) < SPLIT_THRESHOLD
}

// ---------------------------------------------------------------------------
// Roots
// ---------------------------------------------------------------------------

/// The identity of one whole tree, bound to its workspace (D-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentRoot {
    /// `blake3(b"aex.tree.root.v1" || workspace || root page || entries || bytes)`.
    pub digest: [u8; 32],
    /// How many leaves the tree holds.
    pub entries: u64,
    /// How many logical bytes the tree holds.
    pub logical_bytes: u64,
}

/// A fully built tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltTree {
    /// The digest of the top page.
    pub root_page: PageDigest,
    /// Every page written, deepest level first, root page last.
    pub pages: Vec<TreeNode>,
    /// How many leaves the tree holds.
    pub entries: u64,
    /// How many logical bytes the tree holds.
    pub logical_bytes: u64,
}

impl BuiltTree {
    /// Every page paired with its digest.
    #[must_use]
    pub fn digested_pages(&self) -> Vec<(PageDigest, &TreeNode)> {
        self.pages
            .iter()
            .map(|page| (page_digest(page), page))
            .collect()
    }

    /// How many levels the tree has. Always at least one.
    #[must_use]
    pub fn levels(&self) -> usize {
        let mut levels = 1;
        let mut current = self.pages.last();
        while let Some(TreeNode::Branch(_)) = current {
            levels += 1;
            let Some(TreeNode::Branch(page)) = current else {
                break;
            };
            let Some(first) = page.children.first() else {
                break;
            };
            current = self
                .pages
                .iter()
                .find(|candidate| page_digest(candidate) == first.child);
        }
        levels
    }
}

/// Builds the canonical tree for an entry set.
///
/// The input need not be sorted: it is ordered here, so the result depends on
/// the set and never on the insertion order.
///
/// # Errors
///
/// Returns [`TreeError::DuplicatePath`] for a repeated path,
/// [`TreeError::AncestorNotDirectory`] when a recorded ancestor is not a
/// directory, and [`TreeError::PageOverTarget`] when a single entry cannot fit
/// one page.
pub fn build_tree(entries: &[TreeEntry]) -> Result<BuiltTree, TreeError> {
    let mut ordered: BTreeMap<NormalizedPath, EntryNode> = BTreeMap::new();
    for entry in entries {
        if ordered
            .insert(entry.path.clone(), entry.node.clone())
            .is_some()
        {
            return Err(TreeError::DuplicatePath(entry.path.clone()));
        }
    }
    build_from_ordered(&ordered)
}

fn build_from_ordered(
    ordered: &BTreeMap<NormalizedPath, EntryNode>,
) -> Result<BuiltTree, TreeError> {
    check_ancestors(ordered)?;

    let total_entries = ordered.len() as u64;
    let total_bytes = ordered
        .values()
        .map(EntryNode::logical_bytes)
        .try_fold(0_u64, u64::checked_add)
        .ok_or(TreeError::LogicalBytesOverflow)?;

    let mut pages: Vec<TreeNode> = Vec::new();
    let mut level_pages: Vec<TreeNode> = Vec::new();
    let mut current: Vec<TreeEntry> = Vec::new();
    let mut current_bytes: u64 = LEAF_TAG.len() as u64 + 4;

    for (path, node) in ordered {
        let entry = TreeEntry {
            path: path.clone(),
            node: node.clone(),
        };
        let entry_bytes = entry_canonical_len(&entry);
        if entry_bytes + LEAF_TAG.len() as u64 + 4 > TREE_PAGE_TARGET_BYTES {
            return Err(TreeError::PageOverTarget {
                bytes: entry_bytes,
                max: TREE_PAGE_TARGET_BYTES,
            });
        }
        if !current.is_empty() && current_bytes + entry_bytes > TREE_PAGE_TARGET_BYTES {
            level_pages.push(TreeNode::Leaf(LeafPage {
                entries: std::mem::take(&mut current),
            }));
            current_bytes = LEAF_TAG.len() as u64 + 4;
        }
        current_bytes += entry_bytes;
        let closes = is_boundary(path);
        current.push(entry);
        if closes {
            level_pages.push(TreeNode::Leaf(LeafPage {
                entries: std::mem::take(&mut current),
            }));
            current_bytes = LEAF_TAG.len() as u64 + 4;
        }
    }
    if !current.is_empty() || level_pages.is_empty() {
        level_pages.push(TreeNode::Leaf(LeafPage { entries: current }));
    }

    let mut level = 0;
    while level_pages.len() > 1 {
        level += 1;
        if level >= MAX_LEVELS {
            return Err(TreeError::TooDeep { max: MAX_LEVELS });
        }
        let children: Vec<BranchChild> = level_pages
            .iter()
            .map(|page| BranchChild {
                separator: page
                    .separator()
                    .cloned()
                    .unwrap_or_else(|| unreachable!("only an empty tree has a page with no entry")),
                child: page_digest(page),
                subtree_entries: page.subtree_entries(),
                subtree_bytes: page.subtree_bytes(),
            })
            .collect();
        pages.append(&mut level_pages);

        let mut branch: Vec<BranchChild> = Vec::new();
        let mut branch_bytes: u64 = BRANCH_TAG.len() as u64 + 4;
        for child in children {
            let child_bytes = child_canonical_len(&child);
            if !branch.is_empty() && branch_bytes + child_bytes > TREE_PAGE_TARGET_BYTES {
                level_pages.push(TreeNode::Branch(BranchPage {
                    children: std::mem::take(&mut branch),
                }));
                branch_bytes = BRANCH_TAG.len() as u64 + 4;
            }
            branch_bytes += child_bytes;
            let closes = boundary_at_level(&child.separator, level);
            branch.push(child);
            if closes {
                level_pages.push(TreeNode::Branch(BranchPage {
                    children: std::mem::take(&mut branch),
                }));
                branch_bytes = BRANCH_TAG.len() as u64 + 4;
            }
        }
        if !branch.is_empty() {
            level_pages.push(TreeNode::Branch(BranchPage { children: branch }));
        }
    }

    let root = level_pages
        .pop()
        .unwrap_or_else(|| unreachable!("the loop always leaves exactly one page"));
    let root_page = page_digest(&root);
    pages.push(root);

    Ok(BuiltTree {
        root_page,
        pages,
        entries: total_entries,
        logical_bytes: total_bytes,
    })
}

fn check_ancestors(ordered: &BTreeMap<NormalizedPath, EntryNode>) -> Result<(), TreeError> {
    for path in ordered.keys() {
        for ancestor in path.ancestors() {
            if let Some(node) = ordered.get(&ancestor)
                && node.kind() != NodeKind::Directory
            {
                return Err(TreeError::AncestorNotDirectory(ancestor));
            }
        }
    }
    Ok(())
}

fn entry_canonical_len(entry: &TreeEntry) -> u64 {
    let mut probe = Vec::new();
    push_len_prefixed(&mut probe, entry.path.as_bytes());
    push_node(&mut probe, &entry.node);
    probe.len() as u64
}

fn child_canonical_len(child: &BranchChild) -> u64 {
    (4 + child.separator.len() + 32 + 8 + 8) as u64
}

/// Binds a built tree to its workspace.
#[must_use]
pub fn root_of(workspace: WorkspaceId, tree: &BuiltTree) -> ContentRoot {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ROOT_TAG);
    hasher.update(workspace.uuid7().as_bytes());
    hasher.update(tree.root_page.as_bytes());
    hasher.update(&tree.entries.to_le_bytes());
    hasher.update(&tree.logical_bytes.to_le_bytes());
    ContentRoot {
        digest: *hasher.finalize().as_bytes(),
        entries: tree.entries,
        logical_bytes: tree.logical_bytes,
    }
}

/// The root of a workspace with no entries.
#[must_use]
pub fn empty_root(workspace: WorkspaceId) -> ContentRoot {
    let tree = build_tree(&[]).unwrap_or_else(|_| unreachable!("an empty entry set always builds"));
    root_of(workspace, &tree)
}

/// Re-derives a root from its pages and rejects any disagreement.
///
/// Every page is re-hashed, every leaf page is checked for order and
/// uniqueness, every branch child is checked against the page it names, and the
/// entry and byte totals are recomputed.
///
/// # Errors
///
/// Returns the [`TreeError`] naming the first disagreement.
pub fn verify_root(
    workspace: WorkspaceId,
    root: &ContentRoot,
    pages: &[TreeNode],
) -> Result<(), TreeError> {
    let mut by_digest: BTreeMap<[u8; 32], &TreeNode> = BTreeMap::new();
    for page in pages {
        by_digest.insert(*page_digest(page).as_bytes(), page);
    }

    let mut root_page: Option<PageDigest> = None;
    for page in pages {
        let digest = page_digest(page);
        match page {
            TreeNode::Leaf(leaf) => {
                let mut seen: Option<&NormalizedPath> = None;
                for entry in &leaf.entries {
                    if let Some(previous) = seen {
                        match previous.cmp_bytes(&entry.path) {
                            std::cmp::Ordering::Less => {}
                            std::cmp::Ordering::Equal => {
                                return Err(TreeError::DuplicatePath(entry.path.clone()));
                            }
                            std::cmp::Ordering::Greater => return Err(TreeError::Unsorted),
                        }
                    }
                    seen = Some(&entry.path);
                }
            }
            TreeNode::Branch(branch) => {
                let mut seen: Option<&NormalizedPath> = None;
                for child in &branch.children {
                    if let Some(previous) = seen
                        && previous.cmp_bytes(&child.separator) != std::cmp::Ordering::Less
                    {
                        return Err(TreeError::Unsorted);
                    }
                    seen = Some(&child.separator);
                    let referenced = by_digest
                        .get(child.child.as_bytes())
                        .ok_or(TreeError::PageNotFound(child.child))?;
                    if referenced.subtree_entries() != child.subtree_entries
                        || referenced.subtree_bytes() != child.subtree_bytes
                    {
                        return Err(TreeError::PageDigestMismatch(child.child));
                    }
                    if referenced.separator() != Some(&child.separator) {
                        return Err(TreeError::PageDigestMismatch(child.child));
                    }
                }
            }
        }
        let referenced_by_any = pages.iter().any(|candidate| match candidate {
            TreeNode::Branch(branch) => branch.children.iter().any(|child| child.child == digest),
            TreeNode::Leaf(_) => false,
        });
        if !referenced_by_any {
            if root_page.is_some() {
                return Err(TreeError::PageDigestMismatch(digest));
            }
            root_page = Some(digest);
        }
    }

    let top = root_page.ok_or(TreeError::RootMetadataMismatch {
        expected: (root.entries, root.logical_bytes),
        computed: (0, 0),
    })?;
    let top_node = by_digest
        .get(top.as_bytes())
        .ok_or(TreeError::PageNotFound(top))?;
    let computed = (top_node.subtree_entries(), top_node.subtree_bytes());
    if computed != (root.entries, root.logical_bytes) {
        return Err(TreeError::RootMetadataMismatch {
            expected: (root.entries, root.logical_bytes),
            computed,
        });
    }
    let recomputed = root_of(
        workspace,
        &BuiltTree {
            root_page: top,
            pages: Vec::new(),
            entries: root.entries,
            logical_bytes: root.logical_bytes,
        },
    );
    if recomputed.digest != root.digest {
        return Err(TreeError::RootMetadataMismatch {
            expected: (root.entries, root.logical_bytes),
            computed,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Views and mutation
// ---------------------------------------------------------------------------

/// A materialized tree: its entry set, its pages and its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeView {
    workspace: WorkspaceId,
    entries: BTreeMap<NormalizedPath, EntryNode>,
    built: BuiltTree,
    root: ContentRoot,
}

impl TreeView {
    /// Materializes an entry set.
    ///
    /// # Errors
    ///
    /// Returns the [`TreeError`] `build_tree` would.
    pub fn build(workspace: WorkspaceId, entries: &[TreeEntry]) -> Result<Self, TreeError> {
        let mut ordered: BTreeMap<NormalizedPath, EntryNode> = BTreeMap::new();
        for entry in entries {
            if ordered
                .insert(entry.path.clone(), entry.node.clone())
                .is_some()
            {
                return Err(TreeError::DuplicatePath(entry.path.clone()));
            }
        }
        Self::from_ordered(workspace, ordered)
    }

    fn from_ordered(
        workspace: WorkspaceId,
        entries: BTreeMap<NormalizedPath, EntryNode>,
    ) -> Result<Self, TreeError> {
        let built = build_from_ordered(&entries)?;
        let root = root_of(workspace, &built);
        Ok(Self {
            workspace,
            entries,
            built,
            root,
        })
    }

    /// The owning workspace.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        self.workspace
    }

    /// The tree's root.
    #[must_use]
    pub const fn root(&self) -> ContentRoot {
        self.root
    }

    /// The built pages.
    #[must_use]
    pub const fn built(&self) -> &BuiltTree {
        &self.built
    }

    /// The entry set, in UTF-8 byte order.
    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<NormalizedPath, EntryNode> {
        &self.entries
    }

    /// The node at a path, when one is recorded.
    #[must_use]
    pub fn get(&self, path: &NormalizedPath) -> Option<&EntryNode> {
        self.entries.get(path)
    }
}

/// One change to a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeMutation {
    /// Create or replace a node.
    Upsert(TreeEntry),
    /// Remove a node. Removing an absent path is a no-op.
    Remove(NormalizedPath),
}

/// What a mutation batch costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDelta {
    /// The root after the batch.
    pub new_root: ContentRoot,
    /// The pages that must be written.
    pub written: Vec<TreeNode>,
    /// The pages the batch leaves unreferenced.
    pub unreferenced: Vec<PageDigest>,
}

/// Applies a mutation batch, writing only the pages that actually change.
///
/// # Errors
///
/// Returns the [`TreeError`] the rebuilt entry set would.
pub fn apply_mutations(
    base: &TreeView,
    mutations: &[TreeMutation],
) -> Result<TreeDelta, TreeError> {
    let mut entries = base.entries.clone();
    for mutation in mutations {
        match mutation {
            TreeMutation::Upsert(entry) => {
                entries.insert(entry.path.clone(), entry.node.clone());
            }
            TreeMutation::Remove(path) => {
                entries.remove(path);
            }
        }
    }
    let next = TreeView::from_ordered(base.workspace, entries)?;

    let before: BTreeSet<[u8; 32]> = base
        .built
        .pages
        .iter()
        .map(|page| *page_digest(page).as_bytes())
        .collect();
    let after: BTreeSet<[u8; 32]> = next
        .built
        .pages
        .iter()
        .map(|page| *page_digest(page).as_bytes())
        .collect();

    let written = next
        .built
        .pages
        .iter()
        .filter(|page| !before.contains(page_digest(page).as_bytes()))
        .cloned()
        .collect();
    let unreferenced = base
        .built
        .pages
        .iter()
        .map(page_digest)
        .filter(|digest| !after.contains(digest.as_bytes()))
        .collect();

    Ok(TreeDelta {
        new_root: next.root,
        written,
        unreferenced,
    })
}

/// Adopts an existing root under a new owner. Writes one pin and copies zero
/// pages.
#[must_use]
pub fn clone_root(
    source: &ContentRoot,
    owner: SessionId,
    kind: RootKind,
    _now: Timestamp,
) -> (ContentRoot, Pin) {
    let adopted = *source;
    (
        adopted,
        Pin::Root {
            session: owner,
            kind,
            root: adopted,
        },
    )
}

/// Every body a page references.
pub fn references(node: &TreeNode) -> impl Iterator<Item = Reference> + '_ {
    let bodies: Vec<Reference> = match node {
        TreeNode::Leaf(page) => page
            .entries
            .iter()
            .filter_map(|entry| entry.node.body().map(|body| Reference::Body(*body)))
            .collect(),
        TreeNode::Branch(page) => page
            .children
            .iter()
            .map(|child| Reference::Page(child.child))
            .collect(),
    };
    bodies.into_iter()
}

/// What a page points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reference {
    /// A file body.
    Body(ContentDigest),
    /// A child page.
    Page(PageDigest),
}

/// Why a tree was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// The same path appeared twice.
    #[error("duplicate path `{0}`")]
    DuplicatePath(NormalizedPath),
    /// A stored page was not in UTF-8 byte order.
    #[error("page entries are not in UTF-8 byte order")]
    Unsorted,
    /// A recorded ancestor was not a directory.
    #[error("ancestor `{0}` is recorded but is not a directory")]
    AncestorNotDirectory(NormalizedPath),
    /// A node kind the tree does not support appeared.
    #[error("unsupported node kind {0:?}")]
    UnsupportedNode(NodeKind),
    /// A branch named a page that was not supplied.
    #[error("page {0} was not supplied")]
    PageNotFound(PageDigest),
    /// A page did not match what its parent recorded about it.
    #[error("page {0} does not match its recorded totals")]
    PageDigestMismatch(PageDigest),
    /// The root's totals disagreed with the pages.
    #[error("root records {expected:?} entries/bytes but the pages compute {computed:?}")]
    RootMetadataMismatch {
        /// What the root claims.
        expected: (u64, u64),
        /// What the pages compute.
        computed: (u64, u64),
    },
    /// One entry could not fit a page.
    #[error("one entry needs {bytes} canonical bytes, above the {max} byte page target")]
    PageOverTarget {
        /// Bytes the entry needs.
        bytes: u64,
        /// The page target.
        max: u64,
    },
    /// The tree needed more levels than the boundary salt supports.
    #[error("tree needs more than {max} levels")]
    TooDeep {
        /// The supported level count.
        max: usize,
    },
    /// The logical byte total overflowed.
    #[error("logical byte total overflowed")]
    LogicalBytesOverflow,
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        BuiltTree, ContentRoot, EntryNode, TreeEntry, TreeError, TreeMutation, TreeNode, TreeView,
        apply_mutations, build_tree, empty_root, root_of, verify_root,
    };
    use crate::digest::ContentDigest;
    use crate::path::{FileMode, NormalizedPath};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_700_000_000_000, [7; 10]))
    }

    fn moment() -> Timestamp {
        Timestamp::from_unix_millis(1_700_000_000_000).expect("in range")
    }

    fn file(path: &str, size: u64) -> TreeEntry {
        TreeEntry {
            path: NormalizedPath::parse(path).expect("valid path"),
            node: EntryNode::File {
                body: ContentDigest::of(path.as_bytes()),
                size_bytes: size,
                mode: FileMode::FILE,
                mtime: moment(),
                media_type: None,
            },
        }
    }

    #[test]
    fn the_empty_tree_has_one_page_and_zero_totals() {
        let tree = build_tree(&[]).expect("builds");
        assert_eq!(tree.pages.len(), 1);
        assert_eq!(tree.entries, 0);
        assert_eq!(tree.logical_bytes, 0);
        let root = empty_root(workspace());
        assert_eq!(root.entries, 0);
        verify_root(workspace(), &root, &tree.pages).expect("verifies");
    }

    #[test]
    fn a_duplicate_path_is_rejected() {
        let entries = vec![file("a", 1), file("a", 2)];
        assert!(matches!(
            build_tree(&entries),
            Err(TreeError::DuplicatePath(_))
        ));
    }

    #[test]
    fn a_recorded_non_directory_ancestor_is_rejected() {
        let entries = vec![file("a", 1), file("a/b", 2)];
        assert!(matches!(
            build_tree(&entries),
            Err(TreeError::AncestorNotDirectory(_))
        ));
    }

    #[test]
    fn a_sparse_tree_builds() {
        let entries = vec![file("a/b/c", 1), file("x/y/z", 2)];
        let tree = build_tree(&entries).expect("builds");
        assert_eq!(tree.entries, 2);
        assert_eq!(tree.logical_bytes, 3);
    }

    #[test]
    fn a_tampered_total_is_caught() {
        let entries: Vec<TreeEntry> = (0..40)
            .map(|index| file(&format!("f{index}"), 10))
            .collect();
        let tree = build_tree(&entries).expect("builds");
        let root = root_of(workspace(), &tree);
        verify_root(workspace(), &root, &tree.pages).expect("verifies");
        let tampered = ContentRoot {
            logical_bytes: root.logical_bytes + 1,
            ..root
        };
        assert!(verify_root(workspace(), &tampered, &tree.pages).is_err());
    }

    #[test]
    fn a_root_is_bound_to_its_workspace() {
        let entries = vec![file("a", 1)];
        let tree = build_tree(&entries).expect("builds");
        let other = WorkspaceId::from_uuid7(Uuid7::compose(1_700_000_000_001, [9; 10]));
        assert_ne!(root_of(workspace(), &tree), root_of(other, &tree));
    }

    #[test]
    fn a_single_entry_mutation_writes_only_the_changed_path() {
        let entries: Vec<TreeEntry> = (0..600)
            .map(|index| file(&format!("dir/file{index:04}"), 10))
            .collect();
        let view = TreeView::build(workspace(), &entries).expect("builds");
        let levels = view.built().levels();
        let delta = apply_mutations(&view, &[TreeMutation::Upsert(file("dir/file0100", 4096))])
            .expect("applies");
        assert!(
            delta.written.len() <= levels + 2,
            "wrote {} pages over {levels} levels",
            delta.written.len()
        );
        assert!(delta.written.len() < view.built().pages.len());
    }

    #[test]
    fn built_tree_levels_counts_the_root_page() {
        let tree: BuiltTree = build_tree(&[file("a", 1)]).expect("builds");
        assert_eq!(tree.levels(), 1);
        assert!(matches!(tree.pages.last(), Some(TreeNode::Leaf(_))));
    }
}
