//! Redaction, tenancy and policy cases for the content object adapter.

mod support;

use aex_content_aws::object_key::{MAX_SIGNATURE_AGE_MILLIS, ObjectKey, PRESIGN_EXPIRY};
use aex_content_aws::object_store::ContentObjectStore;
use aex_content_aws::policy::{DELETE_PRINCIPAL_ROLE, REQUIRED_DENIES};
use aex_content_aws::redacted::RedactedUrl;

use support::{capturing_store, digest, other_workspace, workspace};

#[tokio::test]
async fn a_presigned_url_cannot_be_printed_by_accident_anywhere() {
    let (store, _receiver) = capturing_store();
    let presigned = store
        .presign_get(&ObjectKey::new(workspace(), &digest(1)), None)
        .await
        .expect("presigning is local");

    let debug = format!("{:?}", presigned.url);
    let display = format!("{}", presigned.url);
    let struct_debug = format!("{presigned:?}");
    for rendering in [&debug, &display, &struct_debug] {
        assert!(!rendering.contains("X-Amz-Signature"), "{rendering}");
        assert!(!rendering.contains("X-Amz-Credential"), "{rendering}");
        assert!(!rendering.contains("https://"), "{rendering}");
    }
    assert!(
        presigned.url.expose().contains("X-Amz-Signature"),
        "the signature is still there; it is only unreachable by accident"
    );
}

#[test]
fn the_fingerprint_a_log_may_carry_identifies_without_authorising() {
    let url = RedactedUrl::new("https://bucket/key?X-Amz-Signature=abc");
    assert_eq!(url.fingerprint().len(), 8);
    assert!(!url.fingerprint().contains("abc"));
}

#[test]
fn two_workspaces_can_never_address_the_same_object() {
    let shared = digest(0x42);
    assert_ne!(
        ObjectKey::new(workspace(), &shared),
        ObjectKey::new(other_workspace(), &shared),
        "physical reuse across tenants is both an equality side channel and a \
         shared deletion fate"
    );
}

#[test]
fn a_key_can_never_escape_its_workspace_prefix() {
    let key = ObjectKey::new(workspace(), &digest(1));
    assert!(!key.as_str().contains(".."));
    assert!(!key.as_str().starts_with('/'));
    assert_eq!(
        key.as_str().matches('/').count(),
        3,
        "a fourth separator would let a crafted digest address another prefix"
    );
}

#[test]
fn the_bucket_policy_this_adapter_depends_on_is_stated_rather_than_assumed() {
    let sids: Vec<&str> = REQUIRED_DENIES.iter().map(|deny| deny.sid).collect();
    assert_eq!(
        sids,
        [
            "DenyUnconditionalCreate",
            "DenyUnconditionalDelete",
            "DenyStaleSignature"
        ]
    );
    assert_eq!(DELETE_PRINCIPAL_ROLE, "content-lifecycle-worker");
}

#[test]
fn a_leaked_signature_can_never_outlive_the_grant_that_authorised_it() {
    assert_eq!(
        u128::from(MAX_SIGNATURE_AGE_MILLIS),
        PRESIGN_EXPIRY.as_millis(),
        "OD-17 pins the presign lifetime and the signatureAge deny to the same value"
    );
}

#[tokio::test]
async fn every_object_request_asserts_the_account_that_must_own_the_bucket() {
    let (store, receiver) = capturing_store();
    let _ignored = store.head(&ObjectKey::new(workspace(), &digest(2))).await;
    let request = support::captured(receiver);
    assert_eq!(
        request.header("x-amz-expected-bucket-owner"),
        Some(support::ACCOUNT),
        "a bucket that moved accounts must fail closed rather than answer"
    );
}
