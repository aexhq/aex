//! What the central runtime adapters do when everything works.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_central_aws::pepper::{PepperDirectory, PepperState, SecretsManagerPepperKeystore};
use aex_central_aws::regional::LambdaRegionalControl;
use aex_control_app::ports::{
    ApplyAccountPauseRequest, DeleteWorkspaceRequest, ProvisionWorkspaceRequest,
    RegionalControlPort as _,
};
use aex_control_domain::{Fence, IntentHash};
use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose};
use aex_identity_domain::{PepperVersion, PresentedDigest, verifier};
use aex_wire::types::Region;
use support::{
    FakeDirectory, MATERIAL_B64, OTHER_MATERIAL_B64, SECRET_ID, VERSION_ID, lambda_client, payload,
    run, secret_response, secrets_client,
};
use uuid::Uuid;

fn keystore(
    directory: Arc<FakeDirectory>,
    answers: Vec<(u16, String)>,
) -> SecretsManagerPepperKeystore {
    let (client, _replay) = secrets_client(answers);
    SecretsManagerPepperKeystore::new(client, SECRET_ID, directory as Arc<dyn PepperDirectory>)
}

#[test]
fn the_active_pepper_is_the_version_the_table_names() {
    let directory = FakeDirectory::one(3, PepperState::Active, VERSION_ID);
    let store = keystore(
        directory,
        vec![(
            200,
            secret_response(VERSION_ID, &payload(3, "identity", MATERIAL_B64)),
        )],
    );
    let (version, _) = run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    assert_eq!(version, PepperVersion::new(3));
}

#[test]
fn a_verifier_computed_under_one_version_reproduces_under_that_version() {
    let directory = FakeDirectory::with(vec![aex_central_aws::pepper::PepperRecord {
        version: PepperVersion::new(3),
        purpose: PepperPurpose::Identity,
        state: PepperState::Active,
        secret_ref: VERSION_ID.to_owned(),
    }]);
    let store = keystore(
        Arc::clone(&directory),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(3, "identity", MATERIAL_B64)),
        )],
    );
    let (version, pepper) = run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    let digest = PresentedDigest::of("a-credential");
    let first = verifier(&pepper, &digest);

    let again = run(store.by_version(PepperPurpose::Identity, version)).expect("the same version");
    assert_eq!(
        verifier(&again, &digest).as_bytes(),
        first.as_bytes(),
        "the same version must reproduce the same verifier"
    );
}

#[test]
fn a_resolved_version_costs_one_secrets_manager_call_however_often_it_is_asked_for() {
    let directory = FakeDirectory::one(3, PepperState::Active, VERSION_ID);
    // One scripted answer. A second fetch would exhaust the replay and fail.
    let store = keystore(
        Arc::clone(&directory),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(3, "identity", MATERIAL_B64)),
        )],
    );
    for _ in 0..5 {
        run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    }
    assert_eq!(store.resolved(), 1);
    assert_eq!(
        directory.reads(),
        5,
        "the lifecycle read is never cached; only the material is"
    );
}

#[test]
fn two_peppers_coexist_across_a_rotation() {
    let retiring = "aaaaaaaa-0000-0000-0000-000000000001";
    let directory = FakeDirectory::with(vec![
        aex_central_aws::pepper::PepperRecord {
            version: PepperVersion::new(4),
            purpose: PepperPurpose::Identity,
            state: PepperState::Active,
            secret_ref: VERSION_ID.to_owned(),
        },
        aex_central_aws::pepper::PepperRecord {
            version: PepperVersion::new(3),
            purpose: PepperPurpose::Identity,
            state: PepperState::Retiring,
            secret_ref: retiring.to_owned(),
        },
    ]);
    let store = keystore(
        directory,
        vec![
            (
                200,
                secret_response(VERSION_ID, &payload(4, "identity", MATERIAL_B64)),
            ),
            (
                200,
                secret_response(retiring, &payload(3, "identity", OTHER_MATERIAL_B64)),
            ),
        ],
    );
    let (active, new_pepper) = run(store.active(PepperPurpose::Identity)).expect("the new pepper");
    assert_eq!(active, PepperVersion::new(4));
    let old_pepper = run(store.by_version(PepperPurpose::Identity, PepperVersion::new(3)))
        .expect("the retiring pepper still verifies");

    let digest = PresentedDigest::of("a-credential-minted-before-the-rotation");
    assert_ne!(
        verifier(&new_pepper, &digest).as_bytes(),
        verifier(&old_pepper, &digest).as_bytes(),
        "the two versions must be distinct material, not the same secret read twice"
    );
    assert_eq!(store.resolved(), 2);
}

#[test]
fn the_start_up_probe_names_the_version_it_loaded() {
    let directory = FakeDirectory::one(7, PepperState::Active, VERSION_ID);
    let store = keystore(
        directory,
        vec![(
            200,
            secret_response(VERSION_ID, &payload(7, "identity", MATERIAL_B64)),
        )],
    );
    assert_eq!(
        run(store.probe(PepperPurpose::Identity)).expect("the probe loads"),
        PepperVersion::new(7)
    );
}

#[test]
fn the_startup_verification_set_loads_current_and_retiring_material() {
    let retiring = "aaaaaaaa-0000-0000-0000-000000000001";
    let directory = FakeDirectory::with(vec![
        aex_central_aws::pepper::PepperRecord {
            version: PepperVersion::new(4),
            purpose: PepperPurpose::Cursor,
            state: PepperState::Active,
            secret_ref: VERSION_ID.to_owned(),
        },
        aex_central_aws::pepper::PepperRecord {
            version: PepperVersion::new(3),
            purpose: PepperPurpose::Cursor,
            state: PepperState::Retiring,
            secret_ref: retiring.to_owned(),
        },
    ]);
    let store = keystore(
        directory,
        vec![
            (
                200,
                secret_response(VERSION_ID, &payload(4, "cursor", MATERIAL_B64)),
            ),
            (
                200,
                secret_response(retiring, &payload(3, "cursor", OTHER_MATERIAL_B64)),
            ),
        ],
    );
    let set = run(store.verification_set(PepperPurpose::Cursor)).expect("the startup set");
    assert_eq!(set.current.0, PepperVersion::new(4));
    assert_eq!(
        set.retiring
            .iter()
            .map(|(version, _)| *version)
            .collect::<Vec<_>>(),
        vec![PepperVersion::new(3)]
    );
    assert_eq!(store.resolved(), 2);
}

fn regional(answers: Vec<support::LambdaAnswer>) -> LambdaRegionalControl {
    let (client, _replay) = lambda_client(answers);
    LambdaRegionalControl::new(
        client,
        BTreeMap::from([(
            Region::EuWest1,
            "arn:aws:lambda:eu-west-1:000000000000:function:regional-control".to_owned(),
        )]),
    )
}

fn provision(workspace: Uuid) -> ProvisionWorkspaceRequest {
    ProvisionWorkspaceRequest {
        workspace_id: workspace,
        organization_id: Uuid::now_v7(),
        region: Region::EuWest1,
        fence: Fence::FIRST,
        intent_hash: IntentHash::from_bytes([9_u8; 32]),
    }
}

#[test]
fn a_region_that_created_the_workspace_answers_created() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        200,
        support::plain(),
        support::provisioned(workspace, true),
    )]);
    let answer = run(port.provision_workspace(&provision(workspace))).expect("the region answers");
    assert_eq!(answer.workspace_id, workspace);
    assert!(answer.created);
}

#[test]
fn a_region_that_already_had_the_workspace_answers_not_created() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        200,
        support::plain(),
        support::provisioned(workspace, false),
    )]);
    let answer = run(port.provision_workspace(&provision(workspace))).expect("the region answers");
    assert!(!answer.created, "a replay is not a second creation");
}

#[test]
fn a_deletion_reports_whether_the_regional_half_is_gone() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        200,
        support::plain(),
        support::deleted(workspace, true),
    )]);
    let answer = run(port.delete_workspace(&DeleteWorkspaceRequest {
        workspace_id: workspace,
        region: Region::EuWest1,
        fence: Fence::FIRST,
    }))
    .expect("the region answers");
    assert!(answer.removed);
}

#[test]
fn an_account_pause_reports_bounded_checkpoint_progress() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        200,
        support::plain(),
        support::account_pause_applied(workspace, false, 7),
    )]);
    let answer = run(port.apply_account_pause(&ApplyAccountPauseRequest {
        workspace_id: workspace,
        organization_id: Uuid::now_v7(),
        region: Region::EuWest1,
        account_epoch: 19,
    }))
    .expect("the region answers");
    assert!(!answer.complete);
    assert_eq!(answer.interrupted, 7);
}

#[test]
fn the_port_reports_exactly_the_regions_it_was_configured_for() {
    let port = regional(Vec::new());
    assert_eq!(port.regions(), vec![Region::EuWest1]);
}
