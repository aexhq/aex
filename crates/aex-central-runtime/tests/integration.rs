//! The pepper keystore against a real Secrets Manager.
//!
//! The scripted suites fix the classification; this one fixes the thing no
//! script can: that a version id minted by the service round-trips, and that a
//! rotation performed the way an operator performs it — `PutSecretValue`,
//! creating a second version — leaves both peppers resolvable at once. That is
//! the whole reason `secret_ref` holds a version id rather than a stage.

mod support;

use std::sync::Arc;

use aex_central_runtime::pepper::{
    PepperDirectory, PepperRecord, PepperState, SecretsManagerPepperKeystore,
};
use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose, StoreError};
use aex_identity_domain::{PepperVersion, PresentedDigest, verifier};
use aex_test_harness::LocalStackContainer;
use aws_sdk_secretsmanager::Client;
use aws_sdk_secretsmanager::config::{BehaviorVersion, Credentials, Region};
use support::{FakeDirectory, MATERIAL_B64, OTHER_MATERIAL_B64, payload};

const SECRET_NAME: &str = "aex/integration/identity-pepper";

fn client(engine: &LocalStackContainer) -> Client {
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
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

fn row(version: u16, state: PepperState, secret_ref: &str) -> PepperRecord {
    PepperRecord {
        version: PepperVersion::new(version),
        purpose: PepperPurpose::Identity,
        state,
        secret_ref: secret_ref.to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_secret_leaves_both_peppers_resolvable_by_their_own_version_ids() {
    let engine = LocalStackContainer::start()
        .await
        .expect("LocalStack starts");
    let client = client(&engine);

    let first = client
        .create_secret()
        .name(SECRET_NAME)
        .secret_string(payload(1, "identity", MATERIAL_B64))
        .send()
        .await
        .expect("the secret is created")
        .version_id
        .expect("a created secret has a version id");

    // Exactly what a rotation does: a second version, same secret.
    let second = client
        .put_secret_value()
        .secret_id(SECRET_NAME)
        .secret_string(payload(2, "identity", OTHER_MATERIAL_B64))
        .send()
        .await
        .expect("the rotation lands")
        .version_id
        .expect("a put has a version id");
    assert_ne!(first, second, "a rotation must mint a distinct version id");

    let directory = FakeDirectory::with(vec![
        row(2, PepperState::Active, &second),
        row(1, PepperState::Retiring, &first),
    ]);
    let store = SecretsManagerPepperKeystore::new(
        client,
        SECRET_NAME,
        directory as Arc<dyn PepperDirectory>,
    );

    let (active, new_pepper) = store
        .active(PepperPurpose::Identity)
        .await
        .expect("the active pepper");
    assert_eq!(active, PepperVersion::new(2));

    let old_pepper = store
        .by_version(PepperPurpose::Identity, PepperVersion::new(1))
        .await
        .expect("the retiring pepper still resolves after the rotation");

    let digest = PresentedDigest::of("a-credential-minted-before-the-rotation");
    assert_ne!(
        verifier(&new_pepper, &digest).as_bytes(),
        verifier(&old_pepper, &digest).as_bytes(),
        "both versions resolved, and to different material"
    );
    assert_eq!(store.resolved(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_id_the_service_does_not_hold_is_not_found() {
    let engine = LocalStackContainer::start()
        .await
        .expect("LocalStack starts");
    let client = client(&engine);
    client
        .create_secret()
        .name("aex/integration/absent-version")
        .secret_string(payload(1, "identity", MATERIAL_B64))
        .send()
        .await
        .expect("the secret is created");

    let directory = FakeDirectory::with(vec![row(
        1,
        PepperState::Active,
        "00000000-0000-0000-0000-000000000000",
    )]);
    let store = SecretsManagerPepperKeystore::new(
        client,
        "aex/integration/absent-version",
        directory as Arc<dyn PepperDirectory>,
    );
    let error = store
        .active(PepperPurpose::Identity)
        .await
        .expect_err("a dangling secret_ref");
    assert_eq!(
        error,
        StoreError::NotFound,
        "a row pointing at nothing is a hard failure, never the current version"
    );
}
