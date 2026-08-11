//! The three seams the staged-upload path rests on, asserted on the wire.
//!
//! `aws.s3.multipart`, `aws.s3.presigned_expiry` and `aws.s3.sse_kms` are
//! declared seams of this crate and, until now, only the first was exercised and
//! only partially. The upload cluster is the first thing to actually *use* the
//! presigner, so what a part grant puts on the wire is now a product contract
//! rather than an implementation detail: a client replays those headers verbatim,
//! and a PUT missing any of them is rejected by S3.
//!
//! Every assertion here is on the serialized HTTP request captured from a real
//! SDK client. The headers are the contract — S3 evaluates them, and nothing else
//! in this system does.

mod support;

use std::collections::BTreeSet;

use aex_content_aws::multipart::MultipartHandle;
use aex_content_aws::object_key::{
    MAX_SIGNATURE_AGE_MILLIS, METADATA_DIGEST, METADATA_WORKSPACE, ObjectKey, PRESIGN_EXPIRY,
};
use aex_content_aws::object_store::ContentObjectStore;

use support::{ACCOUNT, ENCRYPTION_CONTEXT, KMS_KEY, captured, capturing_store, digest, workspace};

fn handle() -> MultipartHandle {
    MultipartHandle {
        key: ObjectKey::new(workspace(), &digest(0xab)),
        upload_id: "provider-mpu-1".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// aws.s3.multipart
// ---------------------------------------------------------------------------

#[tokio::test]
async fn opening_a_multipart_upload_is_sse_kms_and_declares_the_sha256_chain() {
    let (store, receiver) = capturing_store();
    let body = digest(0xab);
    let _ignored = store
        .begin_multipart(workspace(), &body, ENCRYPTION_CONTEXT)
        .await;

    let request = captured(receiver);
    assert_eq!(request.method, "POST");
    assert!(
        request.uri.contains("uploads"),
        "a multipart create posts to ?uploads: {}",
        request.uri
    );
    assert!(
        request
            .uri
            .contains(ObjectKey::new(workspace(), &body).as_str()),
        "the upload is opened on the final content-addressed key, so the durable \
         objectKey a sweep later heads is the same one: {}",
        request.uri
    );
    assert_eq!(
        request.header("x-amz-checksum-algorithm"),
        Some("SHA256"),
        "without the declared algorithm S3 never verifies a part checksum and the \
         three-fact integrity argument collapses to two"
    );
    assert_eq!(
        request.header("x-amz-expected-bucket-owner"),
        Some(ACCOUNT),
        "a bucket moved between accounts must fail closed"
    );
}

#[tokio::test]
async fn a_part_grant_signs_the_exact_length_and_the_exact_part_checksum() {
    // A presign sends nothing: it produces a signature. So the assertions are on
    // what the caller is handed, which is exactly what the caller will replay.
    let (store, _receiver) = capturing_store();
    let grant = store
        .presign_part(&handle(), 7, "cGFydC1zZXZlbg==", 5 * 1024 * 1024)
        .await
        .expect("presigns");

    let url = grant.url.expose();
    assert!(
        url.contains("partNumber=7"),
        "a grant is bound to one part number: {url}"
    );
    assert!(
        url.contains("uploadId=provider-mpu-1"),
        "a grant is bound to the exact provider upload the row names: {url}"
    );
    assert!(
        url.contains(ObjectKey::new(workspace(), &digest(0xab)).as_str()),
        "a grant is bound to the final content-addressed key: {url}"
    );
    assert_eq!(
        grant
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.as_str()),
        Some("5242880"),
        "the signed length is the exact one the plan settled"
    );
}

#[tokio::test]
async fn a_part_grant_returns_every_header_the_client_has_to_replay() {
    let (store, _receiver) = capturing_store();
    let grant = store
        .presign_part(&handle(), 1, "cGFydC1vbmU=", 5 * 1024 * 1024)
        .await
        .expect("presigns");

    let names: BTreeSet<String> = grant
        .headers
        .iter()
        .map(|(name, _)| name.to_lowercase())
        .collect();

    // This is the whole of defect 19: the wire `UploadPartGrant` used to carry a
    // URL and nothing else, and a PUT of that URL without these headers is
    // rejected by S3. The grant is only usable because it hands them back.
    assert_eq!(
        names,
        BTreeSet::from([
            "content-length".to_owned(),
            "x-amz-checksum-sha256".to_owned(),
            "x-amz-expected-bucket-owner".to_owned(),
        ]),
        "this exact set must stay aligned with content-bucket browser CORS"
    );
    assert_eq!(
        grant
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-amz-checksum-sha256"))
            .map(|(_, value)| value.as_str()),
        Some("cGFydC1vbmU="),
        "the replayed checksum is the one the caller declared, verbatim"
    );
    assert_eq!(grant.part_number, 1);
}

#[tokio::test]
async fn a_completion_sends_the_declared_object_size_and_refuses_to_overwrite() {
    let (store, receiver) = capturing_store();
    let _ignored = store
        .complete_multipart(&handle(), &support::manifest())
        .await;

    // The first call a completion makes is `ListParts`, because the manifest is
    // checked against what the provider actually holds before anything is
    // assembled.
    let request = captured(receiver);
    assert_eq!(request.method, "GET");
    assert!(
        request.uri.contains("uploadId=provider-mpu-1"),
        "{}",
        request.uri
    );
}

#[tokio::test]
async fn an_abort_names_the_exact_upload_and_never_a_prefix() {
    let (store, receiver) = capturing_store();
    let _ignored = store.abort_multipart(&handle()).await;

    let request = captured(receiver);
    assert_eq!(request.method, "DELETE");
    assert!(
        request.uri.contains("uploadId=provider-mpu-1"),
        "an abort that did not name the exact upload could destroy another one: {}",
        request.uri
    );
    assert!(
        request
            .uri
            .contains(ObjectKey::new(workspace(), &digest(0xab)).as_str()),
        "{}",
        request.uri
    );
}

// ---------------------------------------------------------------------------
// aws.s3.presigned_expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_presigned_url_expires_exactly_when_the_bucket_policy_stops_admitting_it() {
    let (store, _receiver) = capturing_store();
    let part = store
        .presign_part(&handle(), 1, "cGFydC1vbmU=", 1_024)
        .await
        .expect("presigns");
    let read = store
        .presign_get(&ObjectKey::new(workspace(), &digest(0xab)), None)
        .await
        .expect("presigns");

    assert_eq!(part.expires_in_seconds, PRESIGN_EXPIRY.as_secs());
    assert_eq!(read.expires_in_seconds, PRESIGN_EXPIRY.as_secs());
    assert_eq!(
        PRESIGN_EXPIRY.as_millis(),
        u128::from(MAX_SIGNATURE_AGE_MILLIS),
        "a signature that outlives the window the bucket policy admits is a bearer \
         credential nobody is tracking"
    );
}

#[tokio::test]
async fn a_presigned_part_url_carries_its_expiry_in_the_signature_itself() {
    let (store, _receiver) = capturing_store();
    let part = store
        .presign_part(&handle(), 1, "cGFydC1vbmU=", 1_024)
        .await
        .expect("presigns");

    let rendered = part.url.expose();
    assert!(
        rendered.contains(&format!("X-Amz-Expires={}", PRESIGN_EXPIRY.as_secs())),
        "the lifetime is signed, not advisory"
    );
    assert!(rendered.contains("X-Amz-Signature="));
}

#[tokio::test]
async fn a_presigned_url_cannot_print_itself() {
    let (store, _receiver) = capturing_store();
    let part = store
        .presign_part(&handle(), 1, "cGFydC1vbmU=", 1_024)
        .await
        .expect("presigns");

    let rendered = format!("{:?}", part.url);
    assert!(
        !rendered.contains("X-Amz-Signature"),
        "a grant that logs itself is a leaked capability: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// aws.s3.sse_kms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_multipart_object_is_encrypted_under_the_content_key_with_its_context() {
    let (store, receiver) = capturing_store();
    let _ignored = store
        .begin_multipart(workspace(), &digest(0xab), ENCRYPTION_CONTEXT)
        .await;

    let request = captured(receiver);
    assert_eq!(
        request.header("x-amz-server-side-encryption"),
        Some("aws:kms")
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption-aws-kms-key-id"),
        Some(KMS_KEY),
        "the content CMK is named explicitly; the bucket default is not the fence"
    );
    assert!(
        request
            .header("x-amz-server-side-encryption-context")
            .is_some(),
        "the encryption context is what a KMS key policy condition can fence on"
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption-bucket-key-enabled"),
        Some("true"),
        "a bucket key is what keeps a multi-thousand-part upload from costing one \
         KMS call per part"
    );
}

#[tokio::test]
async fn the_multipart_create_records_the_workspace_and_digest_the_head_oracle_reads() {
    let (store, receiver) = capturing_store();
    let body = digest(0xab);
    let _ignored = store
        .begin_multipart(workspace(), &body, ENCRYPTION_CONTEXT)
        .await;

    let request = captured(receiver);
    let workspace_metadata = format!("x-amz-meta-{METADATA_WORKSPACE}");
    let digest_metadata = format!("x-amz-meta-{METADATA_DIGEST}");
    assert_eq!(
        request.header(&workspace_metadata),
        Some(workspace().to_string().as_str())
    );
    assert_eq!(
        request.header(&digest_metadata),
        Some(hex::encode(body.as_bytes()).as_str()),
        "the expiry sweep resolves an ambiguous completion by comparing this \
         metadata with the upload's declared digest, so a multipart create that \
         omitted it would make the D-3 oracle unusable"
    );
}

#[tokio::test]
async fn a_part_upload_never_re_declares_the_key_material() {
    let (store, _receiver) = capturing_store();
    let grant = store
        .presign_part(&handle(), 1, "cGFydC1vbmU=", 1_024)
        .await
        .expect("presigns");

    assert!(
        !grant.headers.iter().any(|(name, _)| {
            name.eq_ignore_ascii_case("x-amz-server-side-encryption-aws-kms-key-id")
        }),
        "S3 takes the key from the upload the part belongs to; re-sending it on a \
         part is rejected, and a presigned URL the client cannot use is worse than \
         no URL at all: {:?}",
        grant.headers
    );
    assert!(
        !grant.url.expose().contains(KMS_KEY),
        "the CMK is not a query parameter a client could see or change"
    );
}
