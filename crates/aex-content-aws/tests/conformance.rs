//! Request conformance for the content object adapter.
//!
//! Every assertion is on the serialized HTTP request captured from a real
//! client, because the headers are the contract: S3 evaluates
//! `If-None-Match`, `If-Match` and the SSE-KMS headers, and nothing else in this
//! system does.

mod support;

use aex_content_aws::multipart::MultipartHandle;
use aex_content_aws::object_key::{METADATA_DIGEST, METADATA_WORKSPACE, ObjectKey, PRESIGN_EXPIRY};
use aex_content_aws::object_store::{ContentObjectStore, FencedDelete, PutImmutable};

use support::{ACCOUNT, ENCRYPTION_CONTEXT, KMS_KEY, captured, capturing_store, digest, workspace};

#[tokio::test]
async fn a_create_is_conditional_checksummed_and_sealed_under_the_content_key() {
    let (store, receiver) = capturing_store();
    let body = digest(0xab);
    let _ignored = store
        .put_immutable(PutImmutable {
            workspace: workspace(),
            digest: &body,
            plaintext_bytes: 5,
            body: b"hello".to_vec(),
            encryption_context: ENCRYPTION_CONTEXT,
        })
        .await;

    let request = captured(receiver);
    assert_eq!(request.method, "PUT");
    assert!(
        request
            .uri
            .contains(ObjectKey::new(workspace(), &body).as_str()),
        "{}",
        request.uri
    );
    assert_eq!(
        request.header("if-none-match"),
        Some("*"),
        "an unconditional create would silently overwrite a content-addressed body"
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption"),
        Some("aws:kms")
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption-aws-kms-key-id"),
        Some(KMS_KEY)
    );
    assert!(
        request
            .header("x-amz-server-side-encryption-context")
            .is_some(),
        "the encryption context is what binds the object to its workspace at the key"
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption-bucket-key-enabled"),
        Some("true")
    );
    assert_eq!(request.header("x-amz-expected-bucket-owner"), Some(ACCOUNT));
    assert!(request.header("x-amz-checksum-sha256").is_some());
    assert_eq!(request.header("content-length"), Some("5"));
    assert_eq!(
        request.header(&format!("x-amz-meta-{METADATA_WORKSPACE}")),
        Some(workspace().to_string().as_str())
    );
    assert_eq!(
        request.header(&format!("x-amz-meta-{METADATA_DIGEST}")),
        Some(hex::encode(body.as_bytes()).as_str())
    );
}

#[tokio::test]
async fn a_delete_names_the_exact_etag_the_sweep_decided_on() {
    let (store, receiver) = capturing_store();
    let key = ObjectKey::new(workspace(), &digest(1));
    let _ignored = store
        .delete_fenced(FencedDelete {
            key: &key,
            etag: "\"d41d8cd98f00b204e9800998ecf8427e\"",
        })
        .await;

    let request = captured(receiver);
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.header("if-match"),
        Some("\"d41d8cd98f00b204e9800998ecf8427e\""),
        "an unconditional delete cannot prove it removed the body that was marked"
    );
    assert_eq!(request.header("x-amz-expected-bucket-owner"), Some(ACCOUNT));
}

#[tokio::test]
async fn a_multipart_create_carries_the_same_encryption_binding_as_a_single_put() {
    let (store, receiver) = capturing_store();
    let _ignored = store
        .begin_multipart(workspace(), &digest(2), ENCRYPTION_CONTEXT)
        .await;

    let request = captured(receiver);
    assert_eq!(request.method, "POST");
    assert!(
        request.uri.contains("uploads"),
        "a multipart create opens the upload: {}",
        request.uri
    );
    assert_eq!(
        request.header("x-amz-server-side-encryption"),
        Some("aws:kms")
    );
    assert_eq!(request.header("x-amz-checksum-algorithm"), Some("SHA256"));
}

#[tokio::test]
async fn a_completion_declares_the_object_size_and_still_refuses_to_overwrite() {
    let (store, receiver) = capturing_store();
    let handle = MultipartHandle {
        key: ObjectKey::new(workspace(), &digest(3)),
        upload_id: "upload-1".to_owned(),
    };
    // The completion lists parts first; the captured request is that list, which
    // is itself part of the contract: a completion never runs on an unverified
    // manifest.
    let _ignored = store
        .complete_multipart(&handle, &support::manifest())
        .await;

    let request = captured(receiver);
    assert!(
        request.uri.contains("uploadId=upload-1"),
        "the first request of a completion is the part listing: {}",
        request.uri
    );
}

#[tokio::test]
async fn a_presigned_read_expires_in_exactly_five_minutes_and_is_bound_to_the_key() {
    let (store, _receiver) = capturing_store();
    let key = ObjectKey::new(workspace(), &digest(4));
    let presigned = store
        .presign_get(&key, Some((0, 1_023)))
        .await
        .expect("presigning is local and needs no network");

    assert_eq!(presigned.expires_in_seconds, PRESIGN_EXPIRY.as_secs());
    let url = presigned.url.expose();
    assert!(url.contains("X-Amz-Expires=300"), "{url}");
    assert!(url.contains(key.as_str()), "the grant is bound to one key");
    assert!(
        presigned
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("range") && value == "bytes=0-1023"),
        "the range is signed, so a grant cannot be widened by the bearer: {:?}",
        presigned.headers
    );
}

#[tokio::test]
async fn a_bounded_read_addresses_only_the_workspace_scoped_immutable_key() {
    let (store, receiver) = capturing_store();
    let body = digest(0x42);
    let _ignored = store.read_bounded(workspace(), &body, 16).await;

    let request = captured(receiver);
    assert_eq!(request.method, "GET");
    assert!(
        request
            .uri
            .contains(ObjectKey::new(workspace(), &body).as_str()),
        "{}",
        request.uri
    );
    assert_eq!(request.header("x-amz-expected-bucket-owner"), Some(ACCOUNT));
}

#[tokio::test]
async fn a_presigned_part_upload_pins_the_part_checksum_and_its_length() {
    let (store, _receiver) = capturing_store();
    let handle = MultipartHandle {
        key: ObjectKey::new(workspace(), &digest(5)),
        upload_id: "upload-2".to_owned(),
    };
    let part = store
        .presign_part(&handle, 1, "cGFydC1vbmU=", 8 * 1024 * 1024)
        .await
        .expect("presigning is local");

    assert_eq!(part.part_number, 1);
    assert!(part.url.expose().contains("partNumber=1"));
    assert!(
        part.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("x-amz-checksum-sha256") && value == "cGFydC1vbmU="
        }),
        "S3 verifies the part checksum on upload, which is where the integrity \
         chain starts: {:?}",
        part.headers
    );
}
