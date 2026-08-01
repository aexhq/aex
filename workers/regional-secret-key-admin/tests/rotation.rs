//! Key-admin idempotency, exclusivity and startup binding requirements.

use std::collections::BTreeMap;

use aex_wire::ids::{OperationId, PrefixedId, Uuid7};
use aex_wire::types::Region;
use regional_secret_key_admin::{
    AdminOutcome, KeyAdmin, KeyAdminError, StartupBinding, validate_startup,
};

fn operation() -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1, [1; 10]))
}

#[test]
fn create_and_rotate_are_idempotent_and_exclusive() {
    let mut admin = KeyAdmin::new("workspace-1").expect("admin");
    assert_eq!(
        admin.create(1, operation()).expect("create"),
        AdminOutcome::Created
    );
    assert_eq!(
        admin.create(1, operation()).expect("replay"),
        AdminOutcome::AlreadyCurrent
    );
    assert_eq!(admin.current_generation(), Some(1));
    assert_eq!(
        admin.rotate(1, 2, operation()).expect("rotate"),
        AdminOutcome::Rotated
    );
    assert_eq!(admin.current_generation(), Some(2));
    assert_eq!(
        admin.rotate(1, 3, operation()),
        Err(KeyAdminError::ConcurrentRotation)
    );
    assert_eq!(admin.current_generation(), Some(2));
}

#[test]
fn startup_refuses_wrong_table_plane_attestation_and_product_bindings() {
    let valid = StartupBinding {
        keystore_table: "regional-secret-keystore".into(),
        expected_logical_name: "regional-secret-keystore".into(),
        kms_key_arn: "arn:aws:kms:eu-west-1:522921482290:key/secret".into(),
        region: Region::EuWest1,
        account_id: "522921482290".into(),
        attestation: operation().to_string(),
        other_aex_values: BTreeMap::new(),
    };
    assert!(validate_startup(&valid).is_ok());
    let mut wrong_table = valid.clone();
    wrong_table.keystore_table = "product-table".into();
    assert_eq!(
        validate_startup(&wrong_table),
        Err(KeyAdminError::WrongKeystore)
    );
    let mut wrong_region = valid.clone();
    wrong_region.kms_key_arn = "arn:aws:kms:us-east-1:522921482290:key/secret".into();
    assert_eq!(
        validate_startup(&wrong_region),
        Err(KeyAdminError::OffPlaneKey)
    );
    let mut invalid_attestation = valid.clone();
    invalid_attestation.attestation = "ses_not-an-operation".into();
    assert_eq!(
        validate_startup(&invalid_attestation),
        Err(KeyAdminError::InvalidAttestation)
    );
    let mut product = valid;
    product
        .other_aex_values
        .insert("AEX_SESSION_TABLE".into(), "forbidden".into());
    assert_eq!(
        validate_startup(&product),
        Err(KeyAdminError::ProductBindingPresent)
    );
}
