//! Failure-path cases for `regional-secret-keystore`.

mod support;

use aex_secret_keystore_dynamodb::branch_key::{self, RecordError, decode};
use aex_session_dynamodb::attr::s;

use support::active_record;

#[test]
fn every_required_attribute_is_required() {
    for attribute in [
        branch_key::BRANCH_KEY_ID,
        branch_key::TYPE,
        branch_key::ENC,
        branch_key::KMS_ARN,
        branch_key::CREATE_TIME,
        branch_key::HIERARCHY_VERSION,
    ] {
        let mut broken = active_record(1);
        broken.remove(attribute);
        let error = decode(&broken).expect_err("a missing attribute");
        assert!(
            matches!(error, RecordError::Missing { .. }),
            "`{attribute}` was defaulted rather than required: {error}"
        );
    }
}

#[test]
fn wrapped_material_stored_as_text_is_refused_rather_than_coerced() {
    let mut broken = active_record(1);
    broken.insert(branch_key::ENC.to_owned(), s("not-a-blob"));
    assert_eq!(
        decode(&broken).unwrap_err(),
        RecordError::WrongType {
            attribute: branch_key::ENC
        }
    );
}

#[test]
fn a_hierarchy_version_that_is_not_an_integer_is_refused() {
    let mut broken = active_record(1);
    broken.insert(
        branch_key::HIERARCHY_VERSION.to_owned(),
        aws_sdk_dynamodb::types::AttributeValue::N("1.5".to_owned()),
    );
    assert!(decode(&broken).is_err());
}

#[test]
fn an_active_record_without_a_version_pointer_still_decodes() {
    // The pointer is optional in the provider schema, so its absence is a fact
    // about the store rather than a decode failure; an administrator sees
    // `None` and can act on it.
    let mut item = active_record(1);
    item.remove(branch_key::VERSION);
    assert!(decode(&item).expect("decodes").version.is_none());
}
