//! Property catalogue for `aex-content-domain`: plan 04 items 70-87.
//!
//! Every case is named for its catalogue row so a failure names the invariant
//! rather than a line number. No case is ignored, environment-gated or retried.

use std::collections::{BTreeMap, BTreeSet};

use aex_content_domain::{
    ContentDigest, ContentMissing, ContentRoot, DeletionDenial, DenialEpoch, DenialProjection,
    DenialSubject, EntryNode, FileMode, GcEpoch, MissingReason, NormalizedPath, OwnerEdge, Pin,
    PinSet, PinSubject, PlacementClass, ReachableFrom, Remediation, RetainReason, RootKind,
    STAGED_ORPHAN_GRACE_MS, SweepCandidate, SweepDecision, TREE_PAGE_TARGET_BYTES, TreeEntry,
    TreeMutation, TreeNode, TreeView, UnwrapDenied, apply_mutations, build_tree,
    canonical_page_bytes, clone_root, empty_root, placement_for, root_of, sweep_decision,
    unwrap_allowed, verify_root,
};
use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_700_000_000_000, [7; 10]))
}

fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_700_000_000_001, [9; 10]))
}

fn session(tag: u8) -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_700_000_000_000, [tag; 10]))
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

/// Path segments drawn from a small alphabet that still crosses the ASCII,
/// Latin-1 and astral-plane boundaries, so byte order and code-point order
/// disagree somewhere in every generated set.
fn segment() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "a".to_owned(),
        "b".to_owned(),
        "Z".to_owned(),
        "z9".to_owned(),
        "_x".to_owned(),
        "é".to_owned(),
        "ß".to_owned(),
        "中".to_owned(),
        "\u{1F600}".to_owned(),
        "file.txt".to_owned(),
        ".hidden".to_owned(),
    ])
}

fn any_path() -> impl Strategy<Value = NormalizedPath> {
    prop::collection::vec(segment(), 1..4)
        .prop_map(|parts| NormalizedPath::parse(&parts.join("/")).expect("generated path is valid"))
}

fn file_node(seed: u64, size: u64) -> EntryNode {
    EntryNode::File {
        body: ContentDigest::of(&seed.to_le_bytes()),
        size_bytes: size,
        mode: FileMode::FILE,
        mtime: moment(1_700_000_000_000),
        media_type: None,
    }
}

fn entry_strategy() -> impl Strategy<Value = TreeEntry> {
    (any_path(), 0_u64..4096, 0_u64..64).prop_map(|(path, size, seed)| TreeEntry {
        path,
        node: file_node(seed, size),
    })
}

/// A deduplicated entry set with no recorded non-directory ancestor, so every
/// generated set is a legal tree.
fn entry_set(max: usize) -> impl Strategy<Value = Vec<TreeEntry>> {
    prop::collection::vec(entry_strategy(), 0..max).prop_map(|entries| {
        let mut by_path: BTreeMap<NormalizedPath, TreeEntry> = BTreeMap::new();
        for entry in entries {
            by_path.insert(entry.path.clone(), entry);
        }
        let paths: BTreeSet<NormalizedPath> = by_path.keys().cloned().collect();
        by_path
            .into_iter()
            .filter(|(path, _)| path.ancestors().iter().all(|it| !paths.contains(it)))
            .map(|(_, entry)| entry)
            .collect()
    })
}

fn wide_paths(count: usize) -> Vec<TreeEntry> {
    (0..count)
        .map(|index| TreeEntry {
            path: NormalizedPath::parse(&format!("dir/file{index:05}")).expect("valid"),
            node: file_node(u64::try_from(index).expect("bounded"), 16),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 70, 71 — paths
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 70 `path_normalization`.
    #[test]
    fn path_normalization(path in any_path()) {
        let reparsed = NormalizedPath::parse(path.as_str()).expect("idempotent");
        prop_assert_eq!(&reparsed, &path);
        prop_assert_eq!(reparsed.as_str(), path.as_str());
    }

    /// 70 `path_normalization`, rejected class.
    #[test]
    fn path_normalization_rejects(raw in "[^\u{0}]{0,32}") {
        let rejected = raw.is_empty()
            || raw.starts_with('/')
            || raw.ends_with('/')
            || raw.contains("//")
            || raw.contains('\\')
            || raw.split('/').any(|part| part == "." || part == "..")
            || raw.chars().any(|value| (value as u32) < 0x20 || ((value as u32) >= 0x7f && (value as u32) <= 0x9f))
            || raw.len() > 4096
            || raw.split('/').any(|part| part.len() > 255);
        prop_assert_eq!(NormalizedPath::parse(&raw).is_err(), rejected);
    }

    /// 71 `path_order_utf8`.
    #[test]
    fn path_order_utf8(left in any_path(), right in any_path()) {
        prop_assert_eq!(
            left.cmp_bytes(&right),
            left.as_str().as_bytes().cmp(right.as_str().as_bytes())
        );
        prop_assert_eq!(left.cmp(&right), left.cmp_bytes(&right));
    }

    /// 81 `placement_boundary`.
    #[test]
    fn placement_boundary(length in 0_u64..200_000) {
        let class = placement_for(length);
        prop_assert_eq!(class == PlacementClass::Inline, length <= 32_768);
        if length > 0 {
            let smaller = placement_for(length - 1);
            prop_assert!(smaller <= class, "placement_for must be monotone");
        }
    }
}

#[test]
fn placement_boundary_is_exact_at_32768() {
    assert_eq!(placement_for(32_768), PlacementClass::Inline);
    assert_eq!(placement_for(32_769), PlacementClass::Object);
}

#[test]
fn placement_boundary_the_object_locator_never_affects_a_root() {
    // The locator is not an input to any digest: a tree is built from entries
    // and bodies alone, so there is no constructor by which a key could reach a
    // root. This case pins the observable half — two builds of the same entry
    // set agree — and the type system carries the rest.
    let entries = wide_paths(64);
    let first = build_tree(&entries).expect("builds");
    let second = build_tree(&entries).expect("builds");
    assert_eq!(root_of(workspace(), &first), root_of(workspace(), &second));
}

// ---------------------------------------------------------------------------
// 72-80 — the tree
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// 72 `tree_canonical`.
    #[test]
    fn tree_canonical(entries in entry_set(40), shuffle in prop::collection::vec(0_usize..40, 0..40)) {
        let ordered = build_tree(&entries).expect("builds");
        let mut permuted = entries.clone();
        for (index, target) in shuffle.into_iter().enumerate() {
            if index < permuted.len() && target < permuted.len() {
                permuted.swap(index, target);
            }
        }
        let shuffled = build_tree(&permuted).expect("builds");
        prop_assert_eq!(ordered.root_page, shuffled.root_page);
        prop_assert_eq!(
            root_of(workspace(), &ordered),
            root_of(workspace(), &shuffled)
        );
    }

    /// 73 `tree_page_bounds`.
    #[test]
    fn tree_page_bounds(entries in entry_set(64)) {
        let tree = build_tree(&entries).expect("builds");
        for page in &tree.pages {
            let bytes = canonical_page_bytes(page).len() as u64;
            prop_assert!(
                bytes <= TREE_PAGE_TARGET_BYTES,
                "page of {bytes} bytes exceeds the target"
            );
        }
    }

    /// 74 `page_digest_injective`.
    #[test]
    fn page_digest_injective(entries in entry_set(24), flip in 0_usize..24) {
        let tree = build_tree(&entries).expect("builds");
        let root = root_of(workspace(), &tree);
        prop_assume!(!entries.is_empty());
        let index = flip % entries.len();
        let mut mutated = entries.clone();
        let flipped = match &mutated[index].node {
            EntryNode::File { body, size_bytes, mode, mtime, media_type } => {
                let mut raw = *body.as_bytes();
                raw[0] ^= 1;
                EntryNode::File {
                    body: ContentDigest::from_bytes(raw),
                    size_bytes: *size_bytes,
                    mode: *mode,
                    mtime: *mtime,
                    media_type: media_type.clone(),
                }
            }
            other => other.clone(),
        };
        mutated[index].node = flipped;
        let after = build_tree(&mutated).expect("builds");
        prop_assert_ne!(root, root_of(workspace(), &after));
    }

    /// 75 `ancestor_rule`.
    #[test]
    fn ancestor_rule(entries in entry_set(24)) {
        // Every generated set is sparse by construction and must build.
        prop_assert!(build_tree(&entries).is_ok());
        // Recording a file ancestor of an existing entry must be rejected.
        if let Some(deep) = entries.iter().find(|entry| !entry.path.ancestors().is_empty()) {
            let ancestor = deep.path.ancestors().remove(0);
            let mut hostile = entries.clone();
            hostile.push(TreeEntry { path: ancestor, node: file_node(1, 1) });
            prop_assert!(build_tree(&hostile).is_err());
        }
    }

    /// 76 `cow_equivalence` and 78 `verify_root_detects`.
    #[test]
    fn cow_equivalence(entries in entry_set(32), extra in entry_strategy()) {
        let view = TreeView::build(workspace(), &entries).expect("builds");
        let applied = apply_mutations(&view, &[TreeMutation::Upsert(extra.clone())]);

        let mut expected: BTreeMap<NormalizedPath, EntryNode> = entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.node.clone()))
            .collect();
        expected.insert(extra.path.clone(), extra.node.clone());
        let rebuilt: Vec<TreeEntry> = expected
            .into_iter()
            .map(|(path, node)| TreeEntry { path, node })
            .collect();
        // Adding an entry under a recorded file ancestor is illegal, and both
        // paths must agree about that too: the mutation path never accepts a
        // tree the direct build rejects.
        let direct = build_tree(&rebuilt);
        prop_assert_eq!(applied.is_ok(), direct.is_ok());
        if let (Ok(delta), Ok(direct)) = (applied, direct) {
            prop_assert_eq!(delta.new_root, root_of(workspace(), &direct));
            verify_root(workspace(), &delta.new_root, &direct.pages).expect("verifies");
        }
    }

    /// 80 `logical_bytes_conservation`.
    #[test]
    fn logical_bytes_conservation(entries in entry_set(48)) {
        let tree = build_tree(&entries).expect("builds");
        let expected: u64 = entries.iter().map(|entry| entry.node.logical_bytes()).sum();
        prop_assert_eq!(tree.logical_bytes, expected);
        let root = root_of(workspace(), &tree);
        prop_assert_eq!(root.logical_bytes, expected);
        prop_assert_eq!(root.entries, entries.len() as u64);

        // 79 `clone_root_is_o1`: adoption preserves both totals and writes one pin.
        let (adopted, pin) = clone_root(&root, session(3), RootKind::CloneSource, moment(1));
        prop_assert_eq!(adopted, root);
        prop_assert!(matches!(pin, Pin::Root { .. }), "clone_root writes a root pin");
    }
}

#[test]
fn tree_page_bounds_forces_a_split_before_the_target() {
    // 73: a wide, deliberately non-boundary-heavy set still respects the target.
    let entries = wide_paths(4_000);
    let tree = build_tree(&entries).expect("builds");
    for page in &tree.pages {
        assert!(canonical_page_bytes(page).len() as u64 <= TREE_PAGE_TARGET_BYTES);
    }
    assert!(tree.pages.len() > 1, "4000 entries must not fit one page");
}

#[test]
fn cow_cost_is_logarithmic_not_linear() {
    // 77 `cow_cost`.
    let entries = wide_paths(3_000);
    let view = TreeView::build(workspace(), &entries).expect("builds");
    let levels = view.built().levels();
    let total = view.built().pages.len();
    let changed = TreeEntry {
        path: NormalizedPath::parse("dir/file01500").expect("valid"),
        node: file_node(9_999, 4_096),
    };
    let delta = apply_mutations(&view, &[TreeMutation::Upsert(changed)]).expect("applies");
    assert!(
        delta.written.len() <= levels + 2,
        "wrote {} pages over {levels} levels",
        delta.written.len()
    );
    assert!(
        delta.written.len() * 4 < total,
        "wrote {} of {total} pages",
        delta.written.len()
    );
}

#[test]
fn verify_root_detects_a_tampered_page() {
    // 78 `verify_root_detects`.
    let entries = wide_paths(500);
    let tree = build_tree(&entries).expect("builds");
    let root = root_of(workspace(), &tree);
    verify_root(workspace(), &root, &tree.pages).expect("verifies");

    let mut tampered = tree.pages.clone();
    let leaf = tampered
        .iter_mut()
        .find_map(|page| match page {
            TreeNode::Leaf(leaf) if !leaf.entries.is_empty() => Some(leaf),
            _ => None,
        })
        .expect("a leaf page exists");
    leaf.entries[0].node = file_node(4_242, 1);
    assert!(verify_root(workspace(), &root, &tampered).is_err());

    let wrong_total = ContentRoot {
        entries: root.entries + 1,
        ..root
    };
    assert!(verify_root(workspace(), &wrong_total, &tree.pages).is_err());
    assert!(verify_root(other_workspace(), &root, &tree.pages).is_err());
}

#[test]
fn logical_bytes_conservation_ignores_directories_and_symlinks() {
    // 80 `logical_bytes_conservation`, non-file nodes.
    let link_path = NormalizedPath::parse("dir/link").expect("valid");
    let entries = vec![
        TreeEntry {
            path: NormalizedPath::parse("dir").expect("valid"),
            node: EntryNode::Directory {
                mode: FileMode::DIRECTORY,
                mtime: moment(0),
            },
        },
        TreeEntry {
            path: NormalizedPath::parse("dir/body").expect("valid"),
            node: file_node(1, 1_024),
        },
        TreeEntry {
            path: link_path.clone(),
            node: EntryNode::Symlink {
                target: aex_content_domain::RelativeInternalPath::parse(&link_path, "body")
                    .expect("valid"),
                mode: FileMode::SYMLINK,
                mtime: moment(0),
            },
        },
    ];
    let tree = build_tree(&entries).expect("builds");
    assert_eq!(tree.logical_bytes, 1_024);
    assert_eq!(tree.entries, 3);
}

#[test]
fn empty_root_is_workspace_bound() {
    assert_ne!(empty_root(workspace()), empty_root(other_workspace()));
    assert_eq!(empty_root(workspace()).entries, 0);
}

// ---------------------------------------------------------------------------
// 82-84 — pins and the sweep
// ---------------------------------------------------------------------------

fn candidate(reachable: Vec<ReachableFrom>, staged_at: i64) -> SweepCandidate {
    SweepCandidate {
        workspace: workspace(),
        digest: ContentDigest::of(b"body"),
        key: None,
        staged_at: moment(staged_at),
        observed_epoch: GcEpoch(5),
        reachable_from: reachable,
    }
}

fn every_pin_kind() -> Vec<Pin> {
    let root = ContentRoot {
        digest: [3; 32],
        entries: 1,
        logical_bytes: 8,
    };
    vec![
        Pin::Root {
            session: session(1),
            kind: RootKind::Persisted,
            root,
        },
        Pin::Registry {
            workspace: workspace(),
            kind: aex_content_domain::RegistryKind::File,
            name: aex_wire::ids::ResourceName::parse("readme").expect("valid"),
            revision: aex_content_domain::Revision::FIRST,
        },
        Pin::Grant {
            grant: aex_content_domain::GrantId(Uuid7::compose(1, [4; 10])),
            digest: ContentDigest::of(b"body"),
            expires_at: moment(STAGED_ORPHAN_GRACE_MS * 4),
        },
        Pin::Cursor {
            cursor: aex_content_domain::CursorId(Uuid7::compose(1, [5; 10])),
            root,
            expires_at: moment(STAGED_ORPHAN_GRACE_MS * 4),
        },
        Pin::Operation {
            operation: aex_wire::ids::OperationId::from_uuid7(Uuid7::compose(1, [6; 10])),
            root,
        },
        Pin::Gc {
            epoch: GcEpoch(5),
            digest: ContentDigest::of(b"body"),
        },
    ]
}

#[test]
fn pin_reachability_every_kind_retains_then_releases() {
    // 82 `pin_reachability`.
    let after_grace = moment(STAGED_ORPHAN_GRACE_MS);
    for pin in every_pin_kind() {
        let held: PinSet = [pin.clone()].into_iter().collect();
        let with_pin = sweep_decision(
            &candidate(vec![ReachableFrom::ThroughRoot(pin.clone())], 0),
            GcEpoch(5),
            &held,
            after_grace,
        );
        assert_eq!(
            with_pin,
            SweepDecision::Retain(pin.retain_reason()),
            "{pin:?} must retain"
        );

        let released = sweep_decision(
            &candidate(vec![ReachableFrom::ThroughRoot(pin.clone())], 0),
            GcEpoch(5),
            &PinSet::new(),
            after_grace,
        );
        assert!(
            matches!(released, SweepDecision::DeleteUnderFence { .. }),
            "{pin:?} must release after its last pin is dropped"
        );
    }
}

#[test]
fn gc_grace_holds_for_twenty_four_hours() {
    // 83 `gc_grace`.
    for offset in [0_i64, 1, STAGED_ORPHAN_GRACE_MS - 1] {
        let decision = sweep_decision(
            &candidate(Vec::new(), 0),
            GcEpoch(5),
            &PinSet::new(),
            moment(offset),
        );
        assert_eq!(
            decision,
            SweepDecision::WaitGrace {
                until: moment(STAGED_ORPHAN_GRACE_MS)
            }
        );
    }
    let decision = sweep_decision(
        &candidate(Vec::new(), 0),
        GcEpoch(5),
        &PinSet::new(),
        moment(STAGED_ORPHAN_GRACE_MS),
    );
    assert!(matches!(decision, SweepDecision::DeleteUnderFence { .. }));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 84 `gc_epoch_fence`.
    #[test]
    fn gc_epoch_fence(observed in 0_u64..32, current in 0_u64..32, elapsed in 0_i64..(STAGED_ORPHAN_GRACE_MS * 3)) {
        let mut probe = candidate(Vec::new(), 0);
        probe.observed_epoch = GcEpoch(observed);
        let decision = sweep_decision(&probe, GcEpoch(current), &PinSet::new(), moment(elapsed));
        if observed < current {
            prop_assert_eq!(
                decision,
                SweepDecision::Retain(RetainReason::NewerEpoch { current: GcEpoch(current) })
            );
        } else {
            prop_assert!(
                !matches!(decision, SweepDecision::Retain(RetainReason::NewerEpoch { .. })),
                "a current epoch never reports a newer one"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 85, 86 — deletion denial
// ---------------------------------------------------------------------------

fn edge_for(subject_session: SessionId) -> OwnerEdge {
    OwnerEdge {
        workspace: workspace(),
        subject: PinSubject::Session(subject_session),
        pin: Pin::Root {
            session: subject_session,
            kind: RootKind::Persisted,
            root: ContentRoot {
                digest: [0; 32],
                entries: 0,
                logical_bytes: 0,
            },
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 85 `denial_survives_restore`.
    #[test]
    fn denial_survives_restore(replayed in 0_u64..16, frontier in 0_u64..16, edges in 1_usize..6) {
        let mut projection = DenialProjection::new();
        projection.advance_to(DenialEpoch(replayed));
        // A restored snapshot's owner edges are exactly what an attacker would
        // control; none of them can move the projection forward.
        let restored: Vec<OwnerEdge> = (0..edges)
            .map(|index| edge_for(session(u8::try_from(index).expect("bounded") + 1)))
            .collect();
        for edge in &restored {
            let outcome = unwrap_allowed(Some(edge), &projection, DenialEpoch(frontier));
            if replayed < frontier {
                prop_assert_eq!(
                    outcome,
                    Err(UnwrapDenied::ProjectionBehind {
                        replayed_through: DenialEpoch(replayed),
                        required: DenialEpoch(frontier),
                    })
                );
            } else {
                prop_assert_eq!(outcome, Ok(()));
            }
        }
    }

    /// 86 `denial_monotone`.
    #[test]
    fn denial_monotone(facts in prop::collection::vec((0_u64..16, 1_u8..6), 0..24)) {
        let mut projection = DenialProjection::new();
        let mut seen_high = DenialEpoch::INITIAL;
        let mut denied: BTreeSet<DenialSubject> = BTreeSet::new();
        for (epoch, tag) in facts {
            let subject = DenialSubject::Session(session(tag));
            projection.apply(DeletionDenial {
                subject,
                epoch: DenialEpoch(epoch),
                recorded_at: moment(0),
            });
            denied.insert(subject);
            if DenialEpoch(epoch) > seen_high {
                seen_high = DenialEpoch(epoch);
            }
            prop_assert_eq!(projection.replayed_through(), seen_high);
            for subject in &denied {
                prop_assert!(projection.is_denied(subject), "a denial is never lifted");
            }
        }
    }
}

#[test]
fn denial_survives_restore_denies_the_named_subject() {
    // 85: even a fully caught-up projection denies a listed subject.
    let mut projection = DenialProjection::new();
    projection.apply(DeletionDenial {
        subject: DenialSubject::Session(session(2)),
        epoch: DenialEpoch(4),
        recorded_at: moment(0),
    });
    assert_eq!(
        unwrap_allowed(Some(&edge_for(session(2))), &projection, DenialEpoch(4)),
        Err(UnwrapDenied::SubjectDenied(DenialSubject::Session(
            session(2)
        )))
    );
    assert_eq!(
        unwrap_allowed(None, &projection, DenialEpoch(4)),
        Err(UnwrapDenied::OwnerEdgeAbsent)
    );
}

// ---------------------------------------------------------------------------
// 87 — content_missing
// ---------------------------------------------------------------------------

#[test]
fn content_missing_typed() {
    // 87 `content_missing_typed`.
    for reason in [
        MissingReason::ObjectAbsent,
        MissingReason::ChecksumMismatch,
        MissingReason::PurgedByGc,
    ] {
        for placement in [PlacementClass::Inline, PlacementClass::Object] {
            let missing = ContentMissing {
                workspace: workspace(),
                digest: ContentDigest::of(b"gone"),
                placement,
                reason,
            };
            assert!(!missing.retryable());
            assert_eq!(missing.remediation(), Remediation::Reupload);
            assert_eq!(missing.code(), aex_wire::error::ErrorCode::ContentMissing);
        }
    }
}
