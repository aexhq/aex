//! Shared fixtures for the `aex-secret-keystore-dynamodb` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use aex_secret_keystore_dynamodb::branch_key::{self, BranchKeyId};
use aex_secret_keystore_dynamodb::store::{KeyStoreBinding, KeyStoreReader};
use aex_session_dynamodb::attr::{Item, b, n, s};
use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

pub const TABLE: &str = "dev-eu-west-1-regional-secret-keystore";
pub const KMS_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/keystore";

pub const DEFINITION: &str =
    include_str!("../../../../migrations/regional/tables/regional-secret-keystore.json");

/// The crate's own source, so a conformance case can assert what is **not** in
/// it: this crate implements no write path, and a grep is the only way to prove
/// a negative like that at the unit layer.
pub const STORE_SOURCE: &str = include_str!("../../src/store.rs");

/// The record codec's source, for the same reason.
pub const BRANCH_KEY_SOURCE: &str = include_str!("../../src/branch_key.rs");

#[must_use]
pub fn binding() -> KeyStoreBinding {
    KeyStoreBinding::new(TABLE, KMS_ARN)
}

#[must_use]
pub fn capturing_reader() -> (KeyStoreReader, CaptureRequestReceiver) {
    let (http_client, receiver) = capture_request(None);
    let config = aws_sdk_dynamodb::Config::builder()
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
    (
        KeyStoreReader::new(Client::from_conf(config), binding()),
        receiver,
    )
}

/// The captured request body, parsed as JSON.
///
/// # Panics
///
/// If no request was captured or the body is not JSON.
#[must_use]
pub fn captured_body(receiver: CaptureRequestReceiver) -> serde_json::Value {
    let request = receiver.expect_request();
    let bytes = request
        .body()
        .bytes()
        .expect("the DynamoDB request body is always in memory");
    serde_json::from_slice(bytes).expect("the DynamoDB request body is JSON")
}

#[must_use]
pub fn workspace(byte: u8) -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
}

#[must_use]
pub fn branch_key_id(byte: u8) -> BranchKeyId {
    BranchKeyId::of(workspace(byte))
}

/// One active record, in the provider's own shape.
#[must_use]
pub fn active_record(byte: u8) -> Item {
    let mut item = Item::new();
    item.insert(
        branch_key::BRANCH_KEY_ID.to_owned(),
        s(branch_key_id(byte).as_str().to_owned()),
    );
    item.insert(branch_key::TYPE.to_owned(), s(branch_key::ACTIVE));
    item.insert(branch_key::ENC.to_owned(), b(vec![0xaa; 64]));
    item.insert(branch_key::KMS_ARN.to_owned(), s(KMS_ARN));
    item.insert(
        branch_key::CREATE_TIME.to_owned(),
        s("2026-08-01T12:34:56.789Z"),
    );
    item.insert(branch_key::HIERARCHY_VERSION.to_owned(), n(1));
    item.insert(
        branch_key::VERSION.to_owned(),
        s("branch:version:0192f0ac-0000-7000-8000-000000000001"),
    );
    item
}
