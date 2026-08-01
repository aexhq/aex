//! Shared fixtures for the `aex-content-aws` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_content_aws::multipart::{CompletedPartPlan, CompletionManifest, PartPlan, ProviderPart};
use aex_content_aws::object_store::{BucketBinding, S3ContentObjects};
use aex_wire::ids::{ContentHash, PrefixedId, Uuid7, WorkspaceId};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const BUCKET: &str = "aex-dev-eu-west-1-content";
pub const ACCOUNT: &str = "000000000000";
pub const KMS_KEY: &str = "arn:aws:kms:eu-west-1:000000000000:key/content";

#[must_use]
pub fn binding() -> BucketBinding {
    BucketBinding {
        bucket: BUCKET.to_owned(),
        expected_owner: ACCOUNT.to_owned(),
        kms_key_id: KMS_KEY.to_owned(),
    }
}

#[must_use]
pub fn capturing_client() -> (Client, CaptureRequestReceiver) {
    let (http_client, receiver) = capture_request(None);
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("eu-west-1"))
        .credentials_provider(Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .http_client(http_client)
        .build();
    (Client::from_conf(config), receiver)
}

#[must_use]
pub fn capturing_store() -> (S3ContentObjects, CaptureRequestReceiver) {
    let (client, receiver) = capturing_client();
    (S3ContentObjects::new(client, binding()), receiver)
}

/// The captured request, as a method, a URI and its headers.
///
/// # Panics
///
/// If no request was captured, which means the adapter did not send what the
/// test believes it sent.
#[must_use]
pub fn captured(receiver: CaptureRequestReceiver) -> CapturedRequest {
    let request = receiver.expect_request();
    CapturedRequest {
        method: request.method().to_owned(),
        uri: request.uri().to_owned(),
        headers: request
            .headers()
            .iter()
            .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
            .collect(),
    }
}

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
}

impl CapturedRequest {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

#[must_use]
pub fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

#[must_use]
pub fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [9; 10]))
}

#[must_use]
pub fn digest(byte: u8) -> ContentHash {
    ContentHash::from_bytes([byte; 32])
}

/// A canonical encryption context, as the crypto adapter renders it.
pub const ENCRYPTION_CONTEXT: &[u8] =
    br#"{"aex:domain":"content-inline","aex:plane":"dev","aex:region":"eu-west-1"}"#;

#[must_use]
pub fn manifest() -> CompletionManifest {
    CompletionManifest {
        parts: vec![
            CompletedPartPlan {
                plan: PartPlan {
                    part_number: 1,
                    size: 8 * 1024 * 1024,
                    sha256_base64: "cGFydC1vbmU=".to_owned(),
                },
                etag: "\"one\"".to_owned(),
            },
            CompletedPartPlan {
                plan: PartPlan {
                    part_number: 2,
                    size: 1_024,
                    sha256_base64: "cGFydC10d28=".to_owned(),
                },
                etag: "\"two\"".to_owned(),
            },
        ],
        total_bytes: 8 * 1024 * 1024 + 1_024,
        whole_object_sha256: digest(0xab),
    }
}

#[must_use]
pub fn provider_parts() -> Vec<ProviderPart> {
    vec![
        ProviderPart {
            part_number: 1,
            size: 8 * 1024 * 1024,
            etag: "\"one\"".to_owned(),
            checksum_sha256: Some("cGFydC1vbmU=".to_owned()),
        },
        ProviderPart {
            part_number: 2,
            size: 1_024,
            etag: "\"two\"".to_owned(),
            checksum_sha256: Some("cGFydC10d28=".to_owned()),
        },
    ]
}
