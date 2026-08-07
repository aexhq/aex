//! What the central runtime adapters do when the substrate misbehaves.
//!
//! Every case here fixes one classification. The classification is the whole
//! value of these adapters: the callers above them branch on `Unavailable`
//! versus `Unknown` versus a typed refusal, and a mis-mapped arm is how one lost
//! response becomes two workspaces or one rotated pepper becomes a mass
//! sign-out.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_central_aws::pepper::{PepperDirectory, PepperState, SecretsManagerPepperKeystore};
use aex_central_aws::regional::LambdaRegionalControl;
use aex_control_app::ports::{EffectError, ProvisionWorkspaceRequest, RegionalControlPort as _};
use aex_control_domain::{Fence, IntentHash};
use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose, StoreError};
use aex_identity_domain::PepperVersion;
use aex_internal_contracts::control::RegionalRefusal;
use aex_wire::types::Region;
use support::{
    FakeDirectory, MATERIAL_B64, SECRET_ID, VERSION_ID, error_type, lambda_client, payload, plain,
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

/// One `awsJson1.1` error body.
fn service_error(kind: &str) -> String {
    format!(r#"{{"__type":"{kind}","message":"a vendor message"}}"#)
}

#[test]
fn an_absent_secret_version_is_not_found_rather_than_an_empty_pepper() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(400, service_error("ResourceNotFoundException"))],
    );
    assert_eq!(
        run(store.active(PepperPurpose::Identity)).expect_err("no such version"),
        StoreError::NotFound
    );
}

#[test]
fn a_denied_role_is_a_privilege_failure_rather_than_an_invalid_credential() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(400, service_error("AccessDeniedException"))],
    );
    assert_eq!(
        run(store.active(PepperPurpose::Identity)).expect_err("denied"),
        StoreError::PermissionDenied
    );
}

#[test]
fn a_throttle_is_retryable_rather_than_fatal() {
    for kind in [
        "ThrottlingException",
        "TooManyRequestsException",
        "InternalServiceError",
    ] {
        let store = keystore(
            FakeDirectory::one(1, PepperState::Active, VERSION_ID),
            vec![(500, service_error(kind))],
        );
        let error = run(store.active(PepperPurpose::Identity)).expect_err(kind);
        assert_eq!(error, StoreError::Unavailable, "{kind}");
        assert!(error.retryable(), "{kind}");
    }
}

#[test]
fn a_secret_version_holding_another_version_s_material_is_refused() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(2, "identity", MATERIAL_B64)),
        )],
    );
    let error = run(store.active(PepperPurpose::Identity)).expect_err("a version mismatch");
    assert!(matches!(error, StoreError::Decode(_)), "{error:?}");
}

#[test]
fn a_cursor_pepper_served_for_an_identity_row_is_refused() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(1, "cursor", MATERIAL_B64)),
        )],
    );
    let error = run(store.active(PepperPurpose::Identity)).expect_err("a purpose mismatch");
    assert!(matches!(error, StoreError::Decode(_)), "{error:?}");
}

#[test]
fn a_retired_version_is_not_found_rather_than_resurrected() {
    let directory = FakeDirectory::one(1, PepperState::Retired, VERSION_ID);
    // No scripted answer at all: reaching Secrets Manager would exhaust the
    // replay, which proves the state check happens before the fetch.
    let store = keystore(directory, Vec::new());
    assert_eq!(
        run(store.by_version(PepperPurpose::Identity, PepperVersion::new(1)))
            .expect_err("a retired pepper"),
        StoreError::NotFound
    );
}

#[test]
fn an_unknown_version_is_not_found_rather_than_the_active_one() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        Vec::new(),
    );
    assert_eq!(
        run(store.by_version(PepperPurpose::Identity, PepperVersion::new(99)))
            .expect_err("an unknown version"),
        StoreError::NotFound,
        "falling back to the active pepper would report a valid credential as invalid"
    );
}

#[test]
fn an_unreachable_directory_is_reported_rather_than_worked_around() {
    let directory = FakeDirectory::one(1, PepperState::Active, VERSION_ID);
    directory.fails_with(StoreError::Unavailable);
    let store = keystore(directory, Vec::new());
    assert_eq!(
        run(store.active(PepperPurpose::Identity)).expect_err("no directory"),
        StoreError::Unavailable
    );
}

#[test]
fn a_secret_payload_that_is_not_the_declared_shape_is_refused() {
    for body in [
        r#"{"version":1}"#,
        r#"{"version":1,"purpose":"identity"}"#,
        "not json at all",
        "[]",
    ] {
        let store = keystore(
            FakeDirectory::one(1, PepperState::Active, VERSION_ID),
            vec![(200, secret_response(VERSION_ID, body))],
        );
        let error = run(store.active(PepperPurpose::Identity)).expect_err(body);
        assert!(matches!(error, StoreError::Decode(_)), "{body}: {error:?}");
    }
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

fn provision(workspace: Uuid, region: Region) -> ProvisionWorkspaceRequest {
    ProvisionWorkspaceRequest {
        workspace_id: workspace,
        organization_id: Uuid::now_v7(),
        region,
        fence: Fence::FIRST,
        intent_hash: IntentHash::from_bytes([9_u8; 32]),
    }
}

#[test]
fn a_handler_that_raised_leaves_the_outcome_unknown() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        200,
        support::function_error(),
        r#"{"errorType":"Error","errorMessage":"boom"}"#.to_owned(),
    )]);
    assert_eq!(
        run(port.provision_workspace(&provision(workspace, Region::EuWest1)))
            .expect_err("the handler raised"),
        EffectError::Unknown,
        "the handler ran; what it did before raising is exactly what reconciliation establishes"
    );
}

#[test]
fn an_answer_this_process_cannot_decode_leaves_the_outcome_unknown() {
    for body in [
        "",
        "not json",
        r#"{"schemaVersion":2,"requestId":"x","payload":{}}"#,
    ] {
        let workspace = Uuid::now_v7();
        let port = regional(vec![(200, plain(), body.to_owned())]);
        assert_eq!(
            run(port.provision_workspace(&provision(workspace, Region::EuWest1))).expect_err(body),
            EffectError::Unknown,
            "{body}"
        );
    }
}

#[test]
fn a_platform_failure_that_may_have_run_the_handler_is_unknown() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(
        500,
        error_type("ServiceException"),
        service_error("ServiceException"),
    )]);
    assert_eq!(
        run(port.provision_workspace(&provision(workspace, Region::EuWest1)))
            .expect_err("a platform failure"),
        EffectError::Unknown
    );
}

#[test]
fn a_refusal_issued_before_the_handler_could_run_is_unavailable() {
    for kind in [
        "ResourceNotFoundException",
        "TooManyRequestsException",
        "RequestTooLargeException",
        "InvalidParameterValueException",
    ] {
        let workspace = Uuid::now_v7();
        let port = regional(vec![(400, error_type(kind), service_error(kind))]);
        assert_eq!(
            run(port.provision_workspace(&provision(workspace, Region::EuWest1))).expect_err(kind),
            EffectError::Unavailable,
            "{kind}"
        );
    }
}

#[test]
fn a_region_this_deployable_was_not_configured_for_is_refused_without_a_call() {
    let workspace = Uuid::now_v7();
    // No scripted answer: dispatching anything would exhaust the replay.
    let port = regional(Vec::new());
    assert_eq!(
        run(port.provision_workspace(&provision(workspace, Region::UsEast1)))
            .expect_err("an unconfigured region"),
        EffectError::Rejected {
            code: "regional_endpoint_not_configured",
            retryable: false
        }
    );
}

#[test]
fn every_wire_refusal_reaches_the_caller_with_its_own_retry_decision() {
    for reason in RegionalRefusal::ALL {
        let workspace = Uuid::now_v7();
        let port = regional(vec![(200, plain(), support::refused(workspace, reason))]);
        assert_eq!(
            run(port.provision_workspace(&provision(workspace, Region::EuWest1)))
                .expect_err(reason.code()),
            EffectError::Rejected {
                code: reason.code(),
                retryable: reason.retryable()
            }
        );
    }
}

#[test]
fn a_region_answering_about_another_workspace_is_never_treated_as_success() {
    let asked_about = Uuid::now_v7();
    let answered_about = Uuid::now_v7();
    let port = regional(vec![(
        200,
        plain(),
        support::provisioned(answered_about, true),
    )]);
    assert_eq!(
        run(port.provision_workspace(&provision(asked_about, Region::EuWest1)))
            .expect_err("a different workspace"),
        EffectError::Rejected {
            code: "regional_answered_another_subject",
            retryable: false
        }
    );
}

#[test]
fn a_deletion_answer_to_a_provision_request_is_refused() {
    let workspace = Uuid::now_v7();
    let port = regional(vec![(200, plain(), support::deleted(workspace, true))]);
    assert_eq!(
        run(port.provision_workspace(&provision(workspace, Region::EuWest1)))
            .expect_err("the wrong answer shape"),
        EffectError::Rejected {
            code: "regional_answered_another_subject",
            retryable: false
        }
    );
}

#[test]
fn a_non_uuid_v7_subject_is_refused_before_any_invoke() {
    let port = regional(Vec::new());
    assert_eq!(
        run(port.provision_workspace(&provision(Uuid::from_u128(4), Region::EuWest1)))
            .expect_err("a v4 row id"),
        EffectError::Rejected {
            code: "regional_subject_is_not_a_uuid_v7",
            retryable: false
        }
    );
}
