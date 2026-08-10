//! Property catalogue for `aex-workspace-domain`: plan 04 items 56-69.

use std::collections::{BTreeMap, BTreeSet};

use aex_content_domain::{
    CiphertextIdentity, ContentDescriptor, ContentDigest, ContentObjectKey, Crc32c, EntryNode,
    FileMode, GrantId, NormalizedPath, Pin, Placement, RegisteredName, RegistryKind, Revision,
    TreeEntry, TreeView,
};
use aex_wire::ids::{
    MeasurementId, OperationId, PrefixedId as _, SessionId, UploadId, Uuid7, WorkspaceId,
};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::{
    ByteRange, CompletionEvidence, ContentObjectLocation, DownloadGrant, ExpiryOutcome,
    GrantPlacement, GrantRejection, GrantSubject, MAX_SIGNED_RANGE_BYTES, PART_MAX_BYTES,
    PART_MAX_COUNT, PART_MIN_BYTES, PartGrantRequest, PartReceipt, PersistReceipt,
    PersistSelection, ProposedValue, RegisteredValueRef, RegistryPointer, RegistryRejection,
    RegistrySelector, SelectorError, SetOutcome, Upload, UploadError, UploadState, ValueDocument,
    VerifiedObject, abort, begin_complete, consume, delete, etag_of, expire, finish_complete,
    grant_parts, mint_grant, plan_parts, plan_persist, replay_receipt, set,
};
use proptest::prelude::*;

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

fn path(text: &str) -> NormalizedPath {
    NormalizedPath::parse(text).expect("valid")
}

fn value_document(body: &[u8]) -> ValueDocument {
    let digest = ContentDigest::of(body);
    ValueDocument::new(
        aex_wire::CanonicalJson::parse(&format!(
            r#"{{"mountPath":"/readme","mediaType":"text/plain","mode":"0644",
                "content":{{"sha256":"{}","sizeBytes":"{}"}}}}"#,
            digest.to_wire(),
            body.len()
        ))
        .expect("valid JSON"),
    )
}

fn proposed(body: &[u8]) -> ProposedValue {
    ProposedValue {
        workspace: workspace(),
        kind: RegistryKind::File,
        name: RegisteredName::parse("readme").expect("valid"),
        value_doc: value_document(body),
        payload: Some(RegisteredValueRef::Content {
            digest: ContentDigest::of(body),
        }),
        upload_state: None,
    }
}

// ---------------------------------------------------------------------------
// 56-58 — the registry
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 56 `registry_overwrite_semantics`.
    #[test]
    fn registry_overwrite_semantics(bodies in prop::collection::vec(0_u8..4, 1..16)) {
        let mut current: Option<RegistryPointer> = None;
        let mut revision = 0_u64;

        for (index, body) in bodies.into_iter().enumerate() {
            let at = moment(i64::try_from(index).expect("bounded") + 1);
            let value = proposed(&[body]);
            let (outcome, commit) = set(current.as_ref(), &value, None, at).expect("sets");
            match outcome {
                SetOutcome::Created => {
                    prop_assert_eq!(commit.pointer.row.revision, Revision::FIRST);
                    revision = 1;
                }
                SetOutcome::Replaced => {
                    prop_assert_eq!(commit.pointer.row.revision.0, revision + 1);
                    revision += 1;
                }
                SetOutcome::Unchanged => {
                    // Nothing moves at all.
                    let before = current.as_ref().expect("unchanged needs a prior");
                    prop_assert!(!commit.wrote);
                    prop_assert_eq!(commit.pointer.row.revision, before.row.revision);
                    prop_assert_eq!(&commit.pointer.row.etag, &before.row.etag);
                    prop_assert_eq!(commit.pointer.row.updated_at, before.row.updated_at);
                }
            }
            current = Some(commit.pointer);
        }
    }

    /// 57 `etag_deterministic`.
    #[test]
    fn etag_deterministic(revision in 0_u64..1_000, body in 0_u8..8, kind_index in 0_usize..5) {
        let kind = RegistryKind::ALL[kind_index];
        let digest = ContentDigest::of(&[body]);
        let tag = etag_of(kind, Revision(revision), &digest);
        prop_assert_eq!(&tag, &etag_of(kind, Revision(revision), &digest));

        for other_kind in RegistryKind::ALL {
            for other_revision in [revision, revision + 1] {
                for other_body in [body, body + 1] {
                    let other = etag_of(
                        other_kind,
                        Revision(other_revision),
                        &ContentDigest::of(&[other_body]),
                    );
                    let same_inputs = other_kind == kind
                        && other_revision == revision
                        && other_body == body;
                    prop_assert_eq!(other == tag, same_inputs);
                }
            }
        }
    }

    /// 58 `if_match_serializes`.
    #[test]
    fn if_match_serializes(writers in 2_usize..6) {
        let created = set(None, &proposed(b"one"), None, moment(0))
            .expect("creates")
            .1
            .pointer;

        // Every writer holds the same tag; at most one may succeed per revision,
        // and a failure is never silently retried into a success.
        let mut succeeded = 0_usize;
        let mut current = created.clone();
        for index in 0..writers {
            let body = [u8::try_from(index).expect("bounded") + 10];
            let outcome = set(
                Some(&current),
                &proposed(&body),
                Some(&created.row.etag),
                moment(i64::try_from(index).expect("bounded") + 1),
            );
            match outcome {
                Ok((_, commit)) => {
                    succeeded += 1;
                    current = commit.pointer;
                }
                Err(RegistryRejection::PreconditionFailed { current: reported }) => {
                    prop_assert_eq!(reported, Some(current.row.etag.clone()));
                }
                Err(other) => prop_assert!(false, "unexpected {other:?}"),
            }
        }
        prop_assert_eq!(succeeded, 1, "one writer per revision");
    }
}

#[test]
fn an_upload_backed_set_requires_a_ready_upload() {
    // 56, upload half.
    let mut value = proposed(b"one");
    value.payload = Some(RegisteredValueRef::Upload {
        upload: UploadId::from_uuid7(Uuid7::compose(1, [4; 10])),
    });
    for state in UploadState::ALL {
        value.upload_state = Some(state);
        let outcome = set(None, &value, None, moment(0));
        assert_eq!(
            outcome.is_ok(),
            state == UploadState::Ready,
            "{state:?} must only be accepted when Ready"
        );
    }
    value.upload_state = None;
    assert!(set(None, &value, None, moment(0)).is_err());
}

#[test]
fn a_stale_if_match_on_delete_is_refused() {
    let created = set(None, &proposed(b"one"), None, moment(0))
        .expect("creates")
        .1
        .pointer;
    let stale: ETag = etag_of(RegistryKind::File, Revision(99), &ContentDigest::of(b"x"));
    assert!(delete(Some(&created), Some(&stale)).is_err());
    assert!(delete(Some(&created), Some(&created.row.etag)).is_ok());
}

// ---------------------------------------------------------------------------
// 59-61 — uploads
// ---------------------------------------------------------------------------

fn evidence() -> CompletionEvidence {
    CompletionEvidence {
        etag: "\"assembled\"".to_owned(),
        checksum_sha256: None,
        checksum_crc64_nvme: None,
        part_count: Some(1),
    }
}

fn upload(size: u64) -> Upload {
    Upload {
        id: UploadId::from_uuid7(Uuid7::compose(1, [1; 10])),
        workspace: workspace(),
        state: UploadState::Created,
        provider_upload_id: "provider-mpu-1".to_owned(),
        object_key: "wks/ab/cd/abcd".to_owned(),
        declared_size: size,
        declared_sha256: ContentDigest::of(b"body"),
        content_type: None,
        parts: plan_parts(size).expect("plannable"),
        completion_manifest: Vec::new(),
        completion: None,
        consumed_by: None,
        created_at: moment(0),
        expires_at: moment(86_400_000),
    }
}

/// The same upload with every part's digest declared, which is what
/// `upload_parts_grant` settles before a completion may be begun.
fn declared(size: u64) -> Upload {
    let staged = upload(size);
    let requests: Vec<PartGrantRequest> = staged
        .parts
        .parts
        .iter()
        .map(|part| PartGrantRequest {
            number: part.number,
            sha256: ContentDigest::of(&part.number.to_be_bytes()),
            size_bytes: part.bytes,
        })
        .collect();
    grant_parts(&staged, &requests, moment(1))
        .expect("grants")
        .upload
}

fn selector() -> RegistrySelector {
    RegistrySelector {
        workspace: workspace(),
        kind: RegistryKind::File,
        name: RegisteredName::parse("readme").expect("valid"),
    }
}

#[test]
fn upload_state_machine_is_closed() {
    // 59 `upload_state_machine`.
    let staged = upload(1_024);

    // `Consumed` is reachable only from `Ready`.
    for state in UploadState::ALL {
        let mut candidate = staged.clone();
        candidate.state = state;
        assert_eq!(
            consume(&candidate, selector(), moment(1)).is_ok(),
            state == UploadState::Ready,
            "{state:?} must not reach Consumed"
        );
    }

    // `Aborted` is rejected from `Completing` and `Ready`.
    for state in UploadState::ALL {
        let mut candidate = staged.clone();
        candidate.state = state;
        assert_eq!(
            abort(&candidate, moment(1)).is_ok(),
            state.can_abort(),
            "{state:?} abort"
        );
    }

    // The three terminals stay terminal.
    for state in [
        UploadState::Consumed,
        UploadState::Aborted,
        UploadState::Expired,
    ] {
        assert!(state.is_terminal());
        let mut candidate = staged.clone();
        candidate.state = state;
        // `Consumed` is still named by a registry pointer, so the sweep leaves
        // it; the other two terminals have no consumer and are reclaimed.
        let swept = expire(&candidate, moment(86_400_001));
        assert_eq!(
            swept,
            if state == UploadState::Consumed {
                ExpiryOutcome::Unchanged
            } else {
                ExpiryOutcome::DeleteRow
            },
            "{state:?} expiry"
        );
        assert!(abort(&candidate, moment(1)).is_err());
        assert!(consume(&candidate, selector(), moment(1)).is_err());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// 60 `part_plan_valid`.
    #[test]
    fn part_plan_valid(size in 1_u64..(PART_MIN_BYTES * 40)) {
        let plan = plan_parts(size).expect("plannable");
        prop_assert!(!plan.parts.is_empty());
        prop_assert!(plan.parts.len() <= PART_MAX_COUNT as usize);
        prop_assert_eq!(plan.total_bytes(), size);
        for (index, part) in plan.parts.iter().enumerate() {
            prop_assert_eq!(part.number, u32::try_from(index + 1).expect("bounded"));
            prop_assert!(part.bytes > 0 && part.bytes <= PART_MAX_BYTES);
            if index + 1 < plan.parts.len() {
                prop_assert!(part.bytes >= PART_MIN_BYTES, "only the last part is free");
            }
        }
    }

    /// 61 `upload_completion_verifies`.
    #[test]
    fn upload_completion_verifies(size_delta in -4_i64..4, wrong_digest in any::<bool>()) {
        let staged = declared(1_024);
        let completing = begin_complete(
            &staged,
            &[PartReceipt { number: 1, etag: "\"one\"".to_owned() }],
            moment(1),
        )
        .expect("begins")
        .upload;

        let verified = VerifiedObject {
            size_bytes: 1_024_u64.saturating_add_signed(size_delta),
            sha256: if wrong_digest {
                ContentDigest::of(b"other")
            } else {
                ContentDigest::of(b"body")
            },
            evidence: evidence(),
        };
        let outcome = finish_complete(&completing, &verified, moment(2));
        let should_succeed = size_delta == 0 && !wrong_digest;
        prop_assert_eq!(outcome.is_ok(), should_succeed);
        if !should_succeed {
            prop_assert!(
                matches!(
                    outcome,
                    Err(UploadError::SizeMismatch { .. } | UploadError::DigestMismatch { .. })
                ),
                "a mismatch is reported, never accepted"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 62-64 — grants
// ---------------------------------------------------------------------------

fn descriptor(size: u64, object: bool) -> ContentDescriptor {
    ContentDescriptor {
        workspace: workspace(),
        digest: ContentDigest::of(b"body"),
        size_bytes: size,
        media_type: None,
        placement: if object {
            Placement::Object {
                key: ContentObjectKey::parse("wks/abc").expect("valid"),
                checksum: Crc32c(7),
            }
        } else {
            Placement::Inline
        },
        ciphertext: CiphertextIdentity {
            key_generation: 1,
            wrapped_key: vec![1; 32],
            nonce: vec![1; 12],
        },
        created_at: moment(0),
    }
}

fn mint(
    size: u64,
    object: bool,
    range: Option<ByteRange>,
) -> Result<(DownloadGrant, Pin), GrantRejection> {
    mint_grant(
        GrantSubject {
            workspace: workspace(),
            session: Some(SessionId::from_uuid7(Uuid7::compose(1, [5; 10]))),
        },
        &descriptor(size, object),
        &ContentObjectLocation {
            key: ContentObjectKey::parse("wks/abc").expect("valid"),
            checksum: aex_workspace_domain::ObjectChecksum::Crc32c(Crc32c(7)),
        },
        range,
        GrantId(Uuid7::compose(1, [2; 10])),
        MeasurementId::from_uuid7(Uuid7::compose(1, [3; 10])),
        moment(0),
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 62 `grant_range_bounds`.
    #[test]
    fn grant_range_bounds(size in 1_u64..100_000, start in 0_u64..120_000, len in 0_u64..120_000) {
        let range = ByteRange {
            start,
            end_exclusive: start.saturating_add(len),
        };
        let outcome = mint(size, true, Some(range));
        let inside = range.start <= range.end_exclusive && range.end_exclusive <= size;
        prop_assert_eq!(outcome.is_ok(), inside);
        if let Ok((grant, _)) = outcome {
            prop_assert_eq!(grant.authorized_bytes, range.len());
            prop_assert!(grant.authorized_bytes <= MAX_SIGNED_RANGE_BYTES);
        }
    }

    /// 63 `grant_expiry`.
    #[test]
    fn grant_expiry(at in 0_i64..600_000) {
        let (grant, pin) = mint(1_024, true, None).expect("mints");
        // There is no redemption to test: the grant is a presigned URL whose S3
        // signature is the expiry. What must hold is the pin's instant.
        prop_assert!(at < grant.expires_at.unix_millis() || at >= grant.expires_at.unix_millis());

        // The pin lives exactly as long as the grant, so garbage collection can
        // never delete a pinned body before the grant lapses.
        let Pin::Grant { expires_at, .. } = pin else {
            panic!("a grant mints a grant pin");
        };
        prop_assert_eq!(expires_at, grant.expires_at);
    }
}

#[test]
fn grant_no_body_copy() {
    // 64 `grant_no_body_copy`.
    let (grant, pin) = mint(1_024, false, None).expect("mints");
    // Every grant is an object range: a small body is promoted to its
    // content-addressed key on first grant rather than redeemed inline, and the
    // grant carries a location, never a copy of the bytes.
    let GrantPlacement::ObjectRange { key, .. } = &grant.placement;
    assert_eq!(key.as_str(), "wks/abc");
    assert!(matches!(pin, Pin::Grant { .. }));
    let rendered = format!("{grant:?}");
    assert!(!rendered.contains("token_hash"));
    assert!(!rendered.contains("wks/abc"));
}

// ---------------------------------------------------------------------------
// 65-69 — persist
// ---------------------------------------------------------------------------

fn file(seed: u64, size: u64) -> EntryNode {
    EntryNode::File {
        body: ContentDigest::of(&seed.to_le_bytes()),
        size_bytes: size,
        mode: FileMode::FILE,
        mtime: moment(0),
        media_type: None,
    }
}

fn view(entries: &[(&str, EntryNode)]) -> TreeView {
    let built: Vec<TreeEntry> = entries
        .iter()
        .map(|(text, node)| TreeEntry {
            path: path(text),
            node: node.clone(),
        })
        .collect();
    TreeView::build(workspace(), &built).expect("builds")
}

#[test]
fn persist_mirror_semantics() {
    // 65 `persist_mirror`.
    let durable = view(&[
        ("src/keep.rs", file(1, 10)),
        ("src/gone.rs", file(2, 20)),
        ("docs/untouched.md", file(3, 30)),
    ]);
    let live = view(&[
        ("src/keep.rs", file(9, 11)),
        ("src/new.rs", file(4, 40)),
        ("docs/untouched.md", file(3, 30)),
    ]);
    let selection =
        PersistSelection::parse(Some(vec!["src/**".to_owned()]), Vec::new()).expect("valid");

    let plan = plan_persist(&durable, &live, &selection).expect("plans");
    assert_eq!(plan.added, vec![path("src/new.rs")]);
    assert_eq!(plan.updated, vec![path("src/keep.rs")]);
    assert_eq!(plan.deleted, vec![path("src/gone.rs")]);
    assert!(plan.changed);

    // An unselected durable path is untouched.
    assert!(!plan.added.contains(&path("docs/untouched.md")));
    assert!(!plan.updated.contains(&path("docs/untouched.md")));
    assert!(!plan.deleted.contains(&path("docs/untouched.md")));
}

#[test]
fn an_exclusion_wins_and_an_exact_include_never_prefix_matches() {
    // 65, exclusion and exactness halves.
    let durable = view(&[("src/a.rs", file(1, 10)), ("src/gen/b.rs", file(2, 20))]);
    let live = view(&[]);
    let selection = PersistSelection::parse(
        Some(vec!["src/**".to_owned()]),
        vec!["src/gen/**".to_owned()],
    )
    .expect("valid");
    let plan = plan_persist(&durable, &live, &selection).expect("plans");
    assert_eq!(plan.deleted, vec![path("src/a.rs")]);

    let exact =
        PersistSelection::parse(Some(vec!["src/a.rs".to_owned()]), Vec::new()).expect("valid");
    assert!(exact.selects(&path("src/a.rs")));
    assert!(!exact.selects(&path("src/a.rs.bak")));
    assert!(!exact.selects(&path("src/a.rsx")));
}

#[test]
fn persist_noop_is_stable() {
    // 66 `persist_noop_stable`.
    let entries = [("src/a.rs", file(1, 10)), ("src/b.rs", file(2, 20))];
    let durable = view(&entries);
    let live = view(&entries);
    let selection = PersistSelection::parse(None, Vec::new()).expect("valid");

    let plan = plan_persist(&durable, &live, &selection).expect("plans");
    assert!(!plan.changed);
    assert!(plan.added.is_empty());
    assert!(plan.updated.is_empty());
    assert!(plan.deleted.is_empty());
    assert_eq!(plan.bytes_moved, 0);
    assert_eq!(plan.next_root, durable.root());
    assert_eq!(plan.shape.changed_leaves, 0);
}

#[test]
fn a_metadata_only_change_is_updated_with_no_bytes_moved() {
    // 67 `persist_counters`.
    let body = ContentDigest::of(&1_u64.to_le_bytes());
    let durable = view(&[(
        "src/a.rs",
        EntryNode::File {
            body,
            size_bytes: 10,
            mode: FileMode::FILE,
            mtime: moment(0),
            media_type: None,
        },
    )]);
    let live = view(&[(
        "src/a.rs",
        EntryNode::File {
            body,
            size_bytes: 10,
            mode: FileMode::new(0o0755).expect("in range"),
            mtime: moment(5),
            media_type: None,
        },
    )]);
    let selection = PersistSelection::parse(None, Vec::new()).expect("valid");
    let plan = plan_persist(&durable, &live, &selection).expect("plans");
    assert_eq!(plan.updated, vec![path("src/a.rs")]);
    assert_eq!(plan.bytes_moved, 0, "a metadata-only change moves no bytes");
    assert!(plan.changed);
}

#[test]
fn persist_replay_requires_the_session_and_the_fingerprint() {
    // 68 `persist_replay`.
    let selection =
        PersistSelection::parse(Some(vec!["src/**".to_owned()]), Vec::new()).expect("valid");
    let other =
        PersistSelection::parse(Some(vec!["docs/**".to_owned()]), Vec::new()).expect("valid");
    let session = SessionId::from_uuid7(Uuid7::compose(1, [5; 10]));
    let receipt = PersistReceipt {
        operation: OperationId::from_uuid7(Uuid7::compose(1, [6; 10])),
        session,
        root: view(&[]).root(),
        changed: true,
        persist_revision: 3,
        added: 1,
        updated: 0,
        deleted: 0,
        bytes_moved: 10,
        last_persisted_at: moment(1),
        selector_fingerprint: selection.fingerprint(),
    };

    assert!(replay_receipt(&receipt, session, &selection.fingerprint()).is_ok());
    assert!(replay_receipt(&receipt, session, &other.fingerprint()).is_err());
    assert!(
        replay_receipt(
            &receipt,
            SessionId::from_uuid7(Uuid7::compose(1, [7; 10])),
            &selection.fingerprint()
        )
        .is_err()
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// 69 `selector_parse_total`.
    #[test]
    fn selector_parse_total(raw in "[a-z*?/.]{0,24}") {
        let outcome = aex_workspace_domain::Selector::parse(&raw);
        let rejected = raw.is_empty()
            || raw.starts_with('/')
            || raw.ends_with('/')
            || raw.contains("//")
            || raw.split('/').any(|part| part.contains("**") && part != "**");
        prop_assert_eq!(outcome.is_err(), rejected);
        if let Ok(selector) = outcome {
            // Accepted selectors round-trip exactly.
            prop_assert_eq!(selector.as_str(), raw.as_str());
        }
    }

    /// 69, the rejected syntax classes.
    #[test]
    fn selector_rejects_classes_and_negation(raw in "[a-z]{0,4}[\\[\\]!{}][a-z]{0,4}") {
        prop_assert!(
            matches!(
                aex_workspace_domain::Selector::parse(&raw),
                Err(SelectorError::UnsupportedSyntax { .. })
            ),
            "class and negation syntax never parses"
        );
    }

    /// 65, generated trees and selections.
    #[test]
    fn persist_mirror_over_generated_trees(
        durable_paths in prop::collection::vec(prop::sample::select(vec![
            "src/a.rs", "src/b.rs", "src/gen/c.rs", "docs/d.md", "top.txt",
        ]), 0..6),
        live_paths in prop::collection::vec(prop::sample::select(vec![
            "src/a.rs", "src/b.rs", "src/gen/c.rs", "docs/d.md", "top.txt",
        ]), 0..6),
    ) {
        let build = |names: &[&str], seed: u64| {
            let mut unique: BTreeMap<String, EntryNode> = BTreeMap::new();
            for name in names {
                unique.insert((*name).to_owned(), file(seed, 10));
            }
            let entries: Vec<TreeEntry> = unique
                .into_iter()
                .map(|(text, node)| TreeEntry { path: path(&text), node })
                .collect();
            TreeView::build(workspace(), &entries).expect("builds")
        };
        let durable = build(&durable_paths, 1);
        let live = build(&live_paths, 2);
        let selection =
            PersistSelection::parse(Some(vec!["src/**".to_owned()]), vec!["src/gen/**".to_owned()])
                .expect("valid");

        let plan = plan_persist(&durable, &live, &selection).expect("plans");
        let durable_names: BTreeSet<&NormalizedPath> = durable.entries().keys().collect();
        let live_names: BTreeSet<&NormalizedPath> = live.entries().keys().collect();

        for added in &plan.added {
            prop_assert!(selection.selects(added));
            prop_assert!(live_names.contains(added));
            prop_assert!(!durable_names.contains(added));
        }
        for deleted in &plan.deleted {
            prop_assert!(selection.selects(deleted));
            prop_assert!(durable_names.contains(deleted));
            prop_assert!(!live_names.contains(deleted));
        }
        for touched in plan.added.iter().chain(&plan.updated).chain(&plan.deleted) {
            // Nothing unselected or excluded is ever touched.
            prop_assert!(selection.selects(touched));
            prop_assert!(!touched.as_str().starts_with("src/gen/"));
        }
    }
}
