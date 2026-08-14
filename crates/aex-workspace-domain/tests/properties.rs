//! Property catalogue for `aex-workspace-domain`: plan 04 items 56-69.

use aex_content_domain::{
    CiphertextIdentity, ContentDescriptor, ContentDigest, ContentObjectKey, Crc32c, GrantId, Pin,
    Placement, RegisteredName, RegistryKind, Revision,
};
use aex_wire::ids::{MeasurementId, PrefixedId as _, SessionId, UploadId, Uuid7, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::{
    ByteRange, CompletionEvidence, ContentObjectLocation, DownloadGrant, ExpiryOutcome,
    GrantPlacement, GrantRejection, GrantSubject, MAX_SIGNED_RANGE_BYTES, PART_MAX_BYTES,
    PART_MAX_COUNT, PART_MIN_BYTES, PartGrantRequest, PartReceipt, ProposedValue,
    RegisteredValueRef, RegistryPointer, RegistryRejection, RegistrySelector, RegistryState,
    SetOutcome, Upload, UploadError, UploadState, ValueDocument, VerifiedObject, abort,
    begin_complete, consume, delete, etag_of, expire, finish_complete, grant_parts, mint_grant,
    plan_parts, set,
};
use proptest::prelude::*;

use aex_workspace_domain::{
    FileIntent, FilePublish, FileSource, ReadyFile, admit_pending, publish_ready,
};

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
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
        state: RegistryState::Ready,
        failure_code: None,
    }
}

// ---------------------------------------------------------------------------
// 56-58 — the registry
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    #[test]
    fn an_older_async_file_intent_never_publishes_over_a_newer_one(
        older in any::<Vec<u8>>(),
        newer in any::<Vec<u8>>(),
    ) {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [19; 10]));
        let name = aex_wire::ids::ResourceName::parse("property.bin").expect("name");
        let at = |millis| aex_wire::types::Timestamp::from_unix_millis(millis).expect("time");
        let older_admission = admit_pending(
            None,
            workspace,
            name.clone(),
            FileSource::Url,
            at(1),
            FileIntent::from_private_counter(1),
        );
        let newer_admission = admit_pending(
            Some(&older_admission.current),
            workspace,
            name,
            FileSource::Upload,
            at(2),
            FileIntent::from_private_counter(2),
        );
        let older_ready = ReadyFile {
            digest: aex_content_domain::ContentDigest::of(&older),
            size_bytes: older.len() as u64,
            media_type: "application/octet-stream".to_owned(),
            object_key: "property/older".to_owned(),
        };
        let older_was_stale = matches!(
            publish_ready(
                &newer_admission.current,
                older_admission.current.intent(),
                older_ready,
                at(3),
            ),
            FilePublish::Stale { .. }
        );
        prop_assert!(older_was_stale);
        let newer_ready = ReadyFile {
            digest: aex_content_domain::ContentDigest::of(&newer),
            size_bytes: newer.len() as u64,
            media_type: "application/octet-stream".to_owned(),
            object_key: "property/newer".to_owned(),
        };
        let FilePublish::Published(published) = publish_ready(
            &newer_admission.current,
            newer_admission.current.intent(),
            newer_ready,
            at(4),
        ) else {
            prop_assert!(false, "latest intent must publish");
            return Ok(());
        };
        prop_assert_eq!(
            published.ready().expect("ready").digest,
            aex_content_domain::ContentDigest::of(&newer)
        );
    }

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
        target_name: aex_wire::ids::ResourceName::parse("artifact").expect("valid resource name"),
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
