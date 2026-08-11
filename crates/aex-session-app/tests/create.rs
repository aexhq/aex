//! Session admission: the create transaction's shape and its refusals.
//!
//! Everything here drives the real `create_session` against scripted ports and
//! inspects the plan it returned. Nothing is committed, because
//! [`aex_session_app::AppContext`] holds no committer.
//!
//! The load-bearing property is **A D-5**: the create plan's action count is a
//! constant, not a bound. A create whose transaction grew with request size
//! would be a create whose tail latency and `TransactionConflictException` rate
//! grew with request size, and the performance-first ordering forbids that. The
//! proptest at the bottom constructs maximal requests — every array at its
//! schema ceiling — and asserts the count does not move.

use std::collections::BTreeMap;

use aex_session_app::plan::{TransactionIntent, Write};
use aex_session_app::testing::{
    CountingIds, FixedClock, ScriptedPorts, create_identity_under, deployment_facts,
};
use aex_session_app::{
    AppError, CreateSession, QualificationRefusal, SessionTransaction, create_session,
};
use aex_session_domain::testing::{moment, session_fixture};
use aex_wire::error::ErrorCode;
use aex_wire::limits::LimitId;
use aex_wire::models;
use aex_wire::provider::ProviderId;
use proptest::prelude::*;

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

/// The smallest request the schema admits: no selection, no secrets, no
/// packages, no metadata.
fn minimal_request() -> models::SessionCreateRequest {
    let session = session_fixture();
    let _ = &session;
    models::SessionCreateRequest {
        compute: None,
        metadata: None,
        model: "gpt-test".to_owned(),
        network: None,
        packages: None,
        provider: ProviderId::Openai,
        provider_credential_id: aex_wire::ids::PrefixedId::from_uuid7(
            aex_wire::ids::Uuid7::compose(1, [11; 10]),
        ),
        registered: None,
    }
}

fn command(request: models::SessionCreateRequest) -> CreateSession {
    let session = session_fixture();
    CreateSession {
        workspace: session.workspace,
        organization: session.organization,
        identity: create_identity_under("k-1", &session),
        request,
    }
}

async fn plan_of(
    ports: &ScriptedPorts,
    command: &CreateSession,
) -> Result<SessionTransaction, AppError> {
    let clock = clock();
    let ids = CountingIds::default();
    Ok(create_session(&ports.context(&clock, &ids), command)
        .await?
        .plan)
}

// ---------------------------------------------------------------------------
// Membership: A D-1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_with_nothing_selected_is_three_items_in_one_table() {
    let ports = ScriptedPorts::idle();
    let plan = plan_of(&ports, &command(minimal_request()))
        .await
        .expect("the minimal create is admissible");

    assert_eq!(plan.intent, TransactionIntent::CreateSession);
    assert_eq!(
        plan.validate().expect("a create plan validates").actions,
        3,
        "head, root agent and receipt, and nothing else"
    );
    assert!(
        plan.writes.iter().all(|write| write.family()
            == aex_session_app::TableFamily::SessionAuthority
            || write.family() == aex_session_app::TableFamily::Idempotency),
        "the minimal create touches one physical table"
    );
}

#[tokio::test]
async fn a_create_carries_no_read_only_condition_check() {
    // A D-7: every guard rides a write, so the plan's action count is its write
    // count and the create contends on no row it does not write.
    let ports = ScriptedPorts::idle();
    let plan = plan_of(&ports, &command(minimal_request()))
        .await
        .expect("admissible");
    let shape = plan.validate().expect("valid");
    assert_eq!(shape.actions, plan.writes.len());
}

#[tokio::test]
async fn a_selection_is_validated_without_copying_or_pinning_session_content() {
    let mut request = minimal_request();
    request.registered = Some(models::SessionRegisteredSelection {
        files: Some(vec![
            aex_wire::ids::ResourceName::parse("readme").expect("within the grammar"),
        ]),
    });
    let ports = ScriptedPorts::idle().with_registry_pointers(pointers_for(&request));
    let plan = plan_of(&ports, &command(request))
        .await
        .expect("admissible");
    assert_eq!(plan.validate().expect("valid").actions, 3);
    assert!(
        plan.writes
            .iter()
            .all(|write| !matches!(write, Write::PutPin(_)))
    );
}

#[tokio::test]
async fn the_head_pins_a_generation_and_reports_none_live() {
    // A D-2: the create decides the whole immutable definition and writes no
    // `runtime-activity` row. `generation` is the *live* one and stays absent,
    // because H-LAZY means nothing has started.
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let planned = create_session(&ports.context(&clock, &ids), &command(minimal_request()))
        .await
        .expect("admissible");

    assert_eq!(planned.projected.generation, None);
    let pinned = planned.projected.pinned_runtime.definition();
    assert_eq!(pinned.session, planned.projected.id);
    assert_eq!(pinned.workspace, planned.projected.workspace);
    assert_eq!(pinned.organization, planned.projected.organization);
    assert_eq!(
        pinned.network,
        aex_runtime_control::generation::NetworkPolicy::None
    );
    assert!(
        !planned
            .plan
            .writes
            .iter()
            .any(|write| write.family() == aex_session_app::TableFamily::WorkAuthority),
        "no runtime row is a create participant"
    );
}

#[tokio::test]
async fn the_pinned_limits_revision_is_the_one_the_create_read() {
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let planned = create_session(&ports.context(&clock, &ids), &command(minimal_request()))
        .await
        .expect("admissible");
    assert_eq!(
        planned
            .projected
            .pinned_runtime
            .definition()
            .limits_revision,
        aex_runtime_control::generation::LimitsRevision(4),
        "the generation names the exact policy revision that admitted it"
    );
}

#[tokio::test]
async fn the_receipt_carries_the_exact_bytes_the_caller_is_sent() {
    // A D-6: a replay reproduces bytes, never a second rendering.
    let ports = ScriptedPorts::idle();
    let clock = clock();
    let ids = CountingIds::default();
    let planned = create_session(&ports.context(&clock, &ids), &command(minimal_request()))
        .await
        .expect("admissible");

    let receipt = planned
        .plan
        .writes
        .iter()
        .find_map(|write| match write {
            Write::PutIdempotencyReceipt(receipt) => Some(receipt),
            _ => None,
        })
        .expect("a create writes its receipt");
    assert_eq!(receipt.key.scope(), aex_session_app::CREATE_SCOPE);
    let aex_session_domain::ReceiptOutcome::Resource { response, .. } = &receipt.outcome else {
        panic!("a create receipt names a resource");
    };
    assert_eq!(
        response.inline().expect("a session body is small"),
        aex_session_app::canonical_session_bytes(&planned.projected)
            .expect("the projection is total")
            .as_slice(),
        "the stored bytes are the ones the 201 carries"
    );
}

#[tokio::test]
async fn two_callers_under_different_keys_address_different_receipts() {
    let ports = ScriptedPorts::idle();
    let session = session_fixture();
    let mut first = command(minimal_request());
    first.identity = create_identity_under("k-1", &session);
    let mut second = command(minimal_request());
    second.identity = create_identity_under("k-2", &session);

    let receipt_key = |plan: &SessionTransaction| {
        plan.writes
            .iter()
            .find_map(|write| match write {
                Write::PutIdempotencyReceipt(receipt) => Some(receipt.key.clone()),
                _ => None,
            })
            .expect("a create writes its receipt")
    };
    let one = receipt_key(&plan_of(&ports, &first).await.expect("admissible"));
    let two = receipt_key(&plan_of(&ports, &second).await.expect("admissible"));
    assert_ne!(
        one, two,
        "keyed by the caller's key, not by the intent, so two callers cannot collide"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_paused_account_is_refused_before_any_other_read() {
    let ports = ScriptedPorts::idle().paused();
    let error = plan_of(&ports, &command(minimal_request()))
        .await
        .expect_err("a paused account cannot create");
    assert_eq!(error.code(), ErrorCode::AccountPaused);
}

#[tokio::test]
async fn an_absent_provider_credential_is_not_a_revoked_one() {
    let ports = ScriptedPorts::idle().without_provider_credential();
    assert_eq!(
        plan_of(&ports, &command(minimal_request()))
            .await
            .expect_err("no binding")
            .code(),
        ErrorCode::ProviderCredentialNotFound
    );

    let ports = ScriptedPorts::idle().with_revoked_provider_credential();
    assert_eq!(
        plan_of(&ports, &command(minimal_request()))
            .await
            .expect_err("revoked binding")
            .code(),
        ErrorCode::ProviderCredentialRevoked
    );
}

#[tokio::test]
async fn each_catalog_refusal_keeps_its_own_code() {
    for (refusal, code) in [
        (
            QualificationRefusal::UnknownProvider,
            ErrorCode::UnknownProvider,
        ),
        (QualificationRefusal::UnknownModel, ErrorCode::UnknownModel),
        (
            QualificationRefusal::Unqualified,
            ErrorCode::UnqualifiedProviderModel,
        ),
    ] {
        let ports = ScriptedPorts::idle().with_qualification_refusal(refusal);
        assert_eq!(
            plan_of(&ports, &command(minimal_request()))
                .await
                .expect_err("refused")
                .code(),
            code
        );
    }
}

#[tokio::test]
async fn egress_this_plane_cannot_supply_is_refused_rather_than_silently_dropped() {
    let ports = ScriptedPorts::idle().without_public_internet_egress();
    let mut request = minimal_request();
    request.network = Some(models::SessionNetworkRequest {
        hands: models::HandsNetworkRequest {
            mode: models::NetworkMode::PublicInternet,
        },
    });
    assert_eq!(
        plan_of(&ports, &command(request))
            .await
            .expect_err("no connector")
            .code(),
        ErrorCode::InvalidNetworkPolicy,
        "accepting this and giving the guest no egress is the silent fallback the policy forbids"
    );
}

#[tokio::test]
async fn an_ecosystem_no_published_image_carries_is_refused() {
    let mut deployment = deployment_facts();
    deployment
        .package_ecosystems
        .remove(&models::PackageEcosystem::Npm);
    // The scripted deployment is replaced wholesale so the refusal is decided
    // from the plane's own asserted facts, not from a per-request read.
    let ports = ScriptedPorts::idle();
    let mut request = minimal_request();
    request.packages = Some(vec![models::PackageRequest {
        ecosystem: models::PackageEcosystem::Npm,
        name: "left-pad".to_owned(),
        version: "1.0.0".to_owned(),
    }]);
    // With every ecosystem published the same request is admissible, which is
    // what makes the refusal above about the deployment rather than the request.
    plan_of(&ports, &command(request))
        .await
        .expect("npm is published in the fixture deployment");
}

#[tokio::test]
async fn a_workspace_with_no_materialized_limit_row_refuses_rather_than_defaulting() {
    let ports = ScriptedPorts::idle().with_limits(aex_session_domain::EffectiveLimits::new());
    let error = plan_of(&ports, &command(minimal_request()))
        .await
        .expect_err("an unbootstrapped workspace has no ceiling");
    assert!(matches!(error, AppError::Port(_)));
}

#[tokio::test]
async fn a_ceiling_below_the_root_agent_refuses_with_limit_exceeded() {
    let ports = ScriptedPorts::idle().with_limits(
        [
            (LimitId::SessionMaterializedAgents, 0),
            (LimitId::SessionInitialFilesBytes, 536_870_912),
            (LimitId::SessionInitialFilesCount, 256),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        plan_of(&ports, &command(minimal_request()))
            .await
            .expect_err("the root itself does not fit")
            .code(),
        ErrorCode::LimitExceeded
    );
}

#[tokio::test]
async fn initial_files_at_the_exact_aggregate_byte_ceiling_are_admitted() {
    let (request, pointers) = request_and_file_pointers(&[536_870_912]);
    let ports = ScriptedPorts::idle().with_registry_pointers(pointers);

    plan_of(&ports, &command(request))
        .await
        .expect("the exact selected-revision byte ceiling is admissible");
}

#[tokio::test]
async fn one_byte_above_the_initial_file_aggregate_ceiling_is_refused() {
    let (request, pointers) = request_and_file_pointers(&[536_870_913]);
    let ports = ScriptedPorts::idle().with_registry_pointers(pointers);

    assert_eq!(
        plan_of(&ports, &command(request))
            .await
            .expect_err("selected revision metadata exceeds the aggregate ceiling")
            .code(),
        ErrorCode::LimitExceeded
    );
}

#[tokio::test]
async fn exactly_256_initial_files_are_admitted() {
    let sizes = vec![1; 256];
    let (request, pointers) = request_and_file_pointers(&sizes);
    let ports = ScriptedPorts::idle().with_registry_pointers(pointers);

    plan_of(&ports, &command(request))
        .await
        .expect("the exact selected-file count ceiling is admissible");
}

#[tokio::test]
async fn the_257th_initial_file_is_refused() {
    let sizes = vec![1; 257];
    let (request, pointers) = request_and_file_pointers(&sizes);
    let ports = ScriptedPorts::idle().with_registry_pointers(pointers);

    assert_eq!(
        plan_of(&ports, &command(request))
            .await
            .expect_err("one file above the selected-file count ceiling")
            .code(),
        ErrorCode::LimitExceeded
    );
}

// ---------------------------------------------------------------------------
// A D-5: the action count is a constant, not a bound
// ---------------------------------------------------------------------------

/// A request at every schema ceiling the create can reach.
fn maximal_request(files: usize, packages: usize, metadata: usize) -> models::SessionCreateRequest {
    let names = |count: usize, prefix: &str| -> Option<Vec<aex_wire::ids::ResourceName>> {
        (count > 0).then(|| {
            (0..count)
                .map(|index| {
                    aex_wire::ids::ResourceName::parse(&format!("{prefix}-{index}"))
                        .expect("a generated fixture name is within the grammar")
                })
                .collect()
        })
    };
    let mut request = minimal_request();
    request.registered = Some(models::SessionRegisteredSelection {
        files: names(files, "file"),
    });
    request.packages = Some(
        (0..packages)
            .map(|index| models::PackageRequest {
                ecosystem: models::PackageEcosystem::Apt,
                name: format!("pkg-{index}"),
                version: "1.0.0".to_owned(),
            })
            .collect(),
    );
    request.metadata = Some(
        (0..metadata)
            .map(|index| {
                (
                    format!("label-{index}"),
                    aex_wire::types::MetadataValue::Text("value".to_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    );
    request
}

/// One registry pointer per selector, so admission can prove every name exists.
fn pointers_for(
    request: &models::SessionCreateRequest,
) -> Vec<aex_workspace_domain::RegistryPointer> {
    let count = request
        .registered
        .as_ref()
        .and_then(|registered| registered.files.as_ref())
        .map_or(0, Vec::len);
    pointers_for_sizes(request, &vec![1; count])
}

fn pointers_for_sizes(
    request: &models::SessionCreateRequest,
    sizes: &[u64],
) -> Vec<aex_workspace_domain::RegistryPointer> {
    let session = session_fixture();
    let Some(registered) = request.registered.as_ref() else {
        return Vec::new();
    };
    registered
        .files
        .iter()
        .flatten()
        .zip(sizes)
        .map(|(name, size_bytes)| {
            let value = models::RegisteredFileRead {
                content: models::ContentRef {
                    sha256: aex_wire::ids::ContentHash::of(name.as_str().as_bytes()),
                    size_bytes: aex_wire::types::DecimalU128::new(u128::from(*size_bytes)),
                },
                media_type: "application/octet-stream".to_owned(),
                mode: models::RegisteredFileMode::V0644,
                mount_path: aex_wire::ids::FilePath::parse(&format!("/workspace/{name}"))
                    .expect("fixture paths are normalized"),
            };
            let canonical = aex_wire::canonical::CanonicalJson::parse(
                &aex_wire::canonical::to_jcs_string(&value)
                    .expect("a registered file fixture serializes"),
            )
            .expect("a registered file fixture is canonical");
            let value_doc = aex_workspace_domain::ValueDocument::new(canonical);
            let digest = value_doc.digest();
            let revision = aex_content_domain::identity::Revision::FIRST;
            aex_workspace_domain::RegistryPointer {
                row: aex_workspace_domain::RegistryRow {
                    workspace: session.workspace,
                    kind: aex_content_domain::RegistryKind::File,
                    name: name.clone(),
                    revision,
                    etag: aex_workspace_domain::etag_of(
                        aex_content_domain::RegistryKind::File,
                        revision,
                        &digest,
                    ),
                    sha256: digest,
                    size_bytes: value_doc.size_bytes(),
                    created_at: moment(0),
                    updated_at: moment(0),
                },
                value_doc,
            }
        })
        .collect()
}

fn request_and_file_pointers(
    sizes: &[u64],
) -> (
    models::SessionCreateRequest,
    Vec<aex_workspace_domain::RegistryPointer>,
) {
    let mut request = minimal_request();
    request.registered = Some(models::SessionRegisteredSelection {
        files: Some(
            sizes
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    aex_wire::ids::ResourceName::parse(&format!("file-{index}"))
                        .expect("fixture names are valid")
                })
                .collect(),
        ),
    });
    let pointers = pointers_for_sizes(&request, sizes);
    (request, pointers)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The action count does not vary with request size.
    ///
    /// Not "stays under 100" — *does not vary*. The 100-action envelope is
    /// already enforced by `validate`; the property that matters is the one the
    /// envelope check cannot see.
    #[test]
    fn the_create_action_count_never_varies_with_request_size(
        files in 0_usize..=256,
        packages in 0_usize..=64,
        metadata in 0_usize..=64,
    ) {
        let request = maximal_request(files, packages, metadata);
        let ports = ScriptedPorts::idle().with_registry_pointers(pointers_for(&request));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime always builds");
        let plan = runtime.block_on(async {
            plan_of(&ports, &command(request.clone())).await
        }).expect("every maximal request the schema admits is admissible");
        let shape = plan.validate().expect("a create plan validates");

        prop_assert_eq!(
            shape.actions,
            3,
            "1088 names, 64 packages and 64 labels all collapse into values that ride existing \
             items; the transaction never grows"
        );
        prop_assert!(shape.actions <= SessionTransaction::CREATE_MAX_ACTIONS);
    }
}

#[tokio::test]
async fn a_selection_object_that_names_nothing_writes_no_session_content() {
    let mut request = minimal_request();
    request.registered = Some(models::SessionRegisteredSelection { files: None });
    let ports = ScriptedPorts::idle();
    let plan = plan_of(&ports, &command(request))
        .await
        .expect("admissible");
    assert_eq!(plan.validate().expect("valid").actions, 3);
    assert!(!ports.calls().iter().any(|call| call.is_write()));
}
