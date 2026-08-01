//! Engine-backed cases for the content object adapter, against `MinIO`.
//!
//! What a real engine proves here: that a multipart part listing walks to
//! exhaustion, that a completion is refused before it is sent when the manifest
//! and the provider disagree, that an aborted upload is idempotent, and that a
//! missing object is `content_missing` rather than an empty read.
//!
//! What `MinIO` cannot prove is asserted as a gap rather than assumed away: it
//! implements no SSE-KMS, no conditional delete, no bucket policy, no
//! `s3:signatureAge` and no AWS checksum semantics. The SSE gap is turned into a
//! positive case — the adapter must refuse rather than quietly store a body
//! unencrypted — and the delete fence carries a case that says out loud why it
//! is a live concern.

mod support;

use aex_content_aws::errors::ContentObjectError;
use aex_content_aws::multipart::{
    CompletedPartPlan, CompletionManifest, MultipartHandle, PartPlan,
};
use aex_content_aws::object_key::ObjectKey;
use aex_content_aws::object_store::{
    BucketBinding, ContentObjectStore, FencedDelete, FencedDeleteOutcome, PutImmutable,
    S3ContentObjects,
};
use aex_test_harness::MinioContainer;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;

use support::{ACCOUNT, BUCKET, ENCRYPTION_CONTEXT, digest, workspace};

const PART_BYTES: usize = 5 * 1024 * 1024;

fn client(engine: &MinioContainer) -> Client {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .force_path_style(true)
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

/// The binding without a KMS key, which is the only shape this engine can
/// serve. Production always carries one; see the SSE case below.
fn engine_binding() -> BucketBinding {
    BucketBinding {
        bucket: BUCKET.to_owned(),
        expected_owner: ACCOUNT.to_owned(),
        kms_key_id: String::new(),
    }
}

async fn engine() -> (MinioContainer, Client) {
    let engine = MinioContainer::start().await.expect("MinIO starts");
    let client = client(&engine);
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("the bucket is created");
    (engine, client)
}

/// Writes an object the way this engine can, so the read-side cases have
/// something to read. The adapter's own create path is asserted separately.
async fn seed(client: &Client, key: &ObjectKey, body: &[u8]) -> String {
    client
        .put_object()
        .bucket(BUCKET)
        .key(key.as_str())
        .body(ByteStream::from(body.to_vec()))
        .metadata(
            aex_content_aws::object_key::METADATA_DIGEST,
            hex::encode(digest(0xab).as_bytes()),
        )
        .send()
        .await
        .expect("the object is written")
        .e_tag
        .expect("an ETag")
}

#[tokio::test]
async fn the_adapter_refuses_to_store_a_body_an_engine_cannot_encrypt() {
    let (_engine, client) = engine().await;
    let store = S3ContentObjects::new(
        client,
        BucketBinding {
            kms_key_id: "arn:aws:kms:eu-west-1:000000000000:key/absent".to_owned(),
            ..engine_binding()
        },
    );

    let body = digest(0xab);
    let error = store
        .put_immutable(PutImmutable {
            workspace: workspace(),
            digest: &body,
            plaintext_bytes: 5,
            body: b"hello".to_vec(),
            encryption_context: ENCRYPTION_CONTEXT,
        })
        .await
        .expect_err("this engine has no KMS");
    assert!(
        !matches!(error, ContentObjectError::DigestCollision { .. }),
        "the refusal must not be mistaken for a collision: {error}"
    );
    // The point is what did **not** happen: no unencrypted fallback object.
    let listed = client_absent(&store).await;
    assert!(listed, "a refused create must leave no object behind");
}

async fn client_absent(store: &S3ContentObjects) -> bool {
    store
        .head(&ObjectKey::new(workspace(), &digest(0xab)))
        .await
        .is_err()
}

#[tokio::test]
async fn a_head_reads_back_the_metadata_a_create_records() {
    let (_engine, client) = engine().await;
    let key = ObjectKey::new(workspace(), &digest(0xab));
    let etag = seed(&client, &key, b"hello").await;

    let store = S3ContentObjects::new(client, engine_binding());
    let head = store.head(&key).await.expect("the object exists");
    assert_eq!(head.content_length, 5);
    assert_eq!(head.etag, etag);
    assert_eq!(
        head.declared_digest,
        Some(hex::encode(digest(0xab).as_bytes())),
        "the digest metadata is what an existing-key comparison is decided on"
    );
}

#[tokio::test]
async fn a_missing_object_is_content_missing_rather_than_an_empty_read() {
    let (_engine, client) = engine().await;
    let store = S3ContentObjects::new(client, engine_binding());
    let error = store
        .head(&ObjectKey::new(workspace(), &digest(0x7f)))
        .await
        .expect_err("no such object");
    assert!(
        matches!(error, ContentObjectError::ContentMissing { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_delete_is_accepted_and_a_repeated_sweep_never_wedges() {
    let (_engine, client) = engine().await;
    let key = ObjectKey::new(workspace(), &digest(0xab));
    let etag = seed(&client, &key, b"hello").await;

    let store = S3ContentObjects::new(client, engine_binding());
    let first = store
        .delete_fenced(FencedDelete {
            key: &key,
            etag: &etag,
        })
        .await
        .expect("the delete is evaluated");
    assert_eq!(first, FencedDeleteOutcome::Deleted);
    assert!(store.head(&key).await.is_err());

    // A retried sweep must never fail the worker.
    store
        .delete_fenced(FencedDelete {
            key: &key,
            etag: &etag,
        })
        .await
        .expect("a repeated delete is an outcome, never an error");
}

#[tokio::test]
async fn this_engine_evaluates_no_delete_precondition_which_is_why_the_fence_is_a_live_concern() {
    // Recorded rather than asserted away: MinIO accepts a `DeleteObject` whose
    // `If-Match` does not match the stored ETag, and answers `204` for a key it
    // does not hold. Both make `FencedDeleteOutcome::Changed` and
    // `AlreadyAbsent` unreachable here. The adapter's half — that it always
    // sends the header — is proved on the serialized request in `conformance`;
    // that the *service* refuses a delete without it is a bucket-policy fact and
    // belongs to the live lane (plan 05 section 8.3 item 7).
    let (_engine, client) = engine().await;
    let key = ObjectKey::new(workspace(), &digest(0xab));
    seed(&client, &key, b"hello").await;

    let store = S3ContentObjects::new(client, engine_binding());
    let outcome = store
        .delete_fenced(FencedDelete {
            key: &key,
            etag: "\"00000000000000000000000000000000\"",
        })
        .await
        .expect("the delete is evaluated");
    assert_eq!(
        outcome,
        FencedDeleteOutcome::Deleted,
        "if this ever reports `Changed`, the engine has gained conditional          deletes and this case should become the real fence assertion"
    );
}

#[tokio::test]
async fn a_part_listing_walks_to_exhaustion_and_a_completion_needs_every_part_to_agree() {
    let (_engine, client) = engine().await;
    let key = ObjectKey::new(workspace(), &digest(0x33));

    let created = client
        .create_multipart_upload()
        .bucket(BUCKET)
        .key(key.as_str())
        .send()
        .await
        .expect("the upload opens");
    let handle = MultipartHandle {
        key: key.clone(),
        upload_id: created.upload_id.expect("an upload id"),
    };

    let mut uploaded = Vec::new();
    for (number, size) in [(1_i32, PART_BYTES), (2, 1_024)] {
        let response = client
            .upload_part()
            .bucket(BUCKET)
            .key(key.as_str())
            .upload_id(&handle.upload_id)
            .part_number(number)
            .body(ByteStream::from(vec![b'a'; size]))
            .send()
            .await
            .expect("the part uploads");
        uploaded.push((number, size, response.e_tag.expect("a part ETag")));
    }

    let store = S3ContentObjects::new(client, engine_binding());
    let listed = store.list_parts(&handle).await.expect("the listing walks");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].part_number, 1);
    assert_eq!(listed[1].part_number, 2);

    // This engine records no part checksum, so a manifest that declares one
    // cannot agree — which is the behaviour that matters: a completion is never
    // sent on an unverified part.
    let manifest = CompletionManifest {
        parts: uploaded
            .iter()
            .map(|(number, size, etag)| CompletedPartPlan {
                plan: PartPlan {
                    part_number: *number,
                    size: u64::try_from(*size).expect("a part size"),
                    sha256_base64: "cGFydA==".to_owned(),
                },
                etag: etag.clone(),
            })
            .collect(),
        total_bytes: u64::try_from(PART_BYTES + 1_024).expect("a total"),
        whole_object_sha256: digest(0x33),
    };
    let error = store
        .complete_multipart(&handle, &manifest)
        .await
        .expect_err("no provider checksum");
    assert!(
        matches!(error, ContentObjectError::IntegrityMismatch { .. }),
        "{error}"
    );

    // The upload is still live: a failed verification never destroys it.
    assert_eq!(
        store
            .list_parts(&handle)
            .await
            .expect("the listing still walks")
            .len(),
        2
    );
    store
        .abort_multipart(&handle)
        .await
        .expect("the abort lands");
    store
        .abort_multipart(&handle)
        .await
        .expect("aborting an absent upload is idempotent");
}

#[tokio::test]
async fn a_presigned_read_is_bound_to_the_key_and_the_range_it_was_minted_for() {
    let (_engine, client) = engine().await;
    let key = ObjectKey::new(workspace(), &digest(0xab));
    seed(&client, &key, b"hello world").await;

    let store = S3ContentObjects::new(client, engine_binding());
    let presigned = store
        .presign_get(&key, Some((0, 4)))
        .await
        .expect("presigning is local");
    assert!(presigned.url.expose().contains(key.as_str()));
    assert!(
        presigned
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("range") && value == "bytes=0-4")
    );
}
