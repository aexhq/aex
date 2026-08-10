//! `SC-CONTROL-WORKSPACE` — an organization, a workspace and an API key that
//! authorizes.
//!
//! This is the scenario every other one stands on: if it fails, nobody can
//! create an organization, nobody can create a workspace, and nobody can obtain
//! a credential, so no later scenario has a principal to run as.
//!
//! # What is real here
//!
//! The committed migration bundle, the committed privilege model, the
//! `aex-control-app` use cases, `AuroraControlStore` and its 61 statements,
//! `AuroraAuthorizationReader`, the real credential mint and the real
//! constant-time verify. The stores connect as `aex_control_api` and `aex_authz`
//! — the roles the two deployables connect as — so a missing grant fails a case.
//!
//! # What is not, and where it bites
//!
//! - **The engine is `PostgreSQL` in a container, not Aurora.** The
//!   `aws.rds_data.transaction` seam stays `requires_live` and is claimed by
//!   `aex-live-central-identity-api`. What runs here is the schema, the
//!   statements and the classification; what does not is the Data `API`
//!   endpoint's own behaviour.
//! - **The regional half is a recording double.** `CreateWorkspace` performs one
//!   effect against `RegionalControlPort`, and this body asserts exactly what
//!   the central plane asks the region for. It cannot assert what the region
//!   then does, and that is the gap the scenario's other four observed artifacts
//!   cover: a workspace with no `workspace_edge_limits` row answers `401
//!   unauthenticated` on every regional request, and the writer for that row
//!   (`aex-session-dynamodb`'s `capacity-limit-projection-write`) has **no
//!   production caller**. The last case below pins the shape of the central
//!   request so that gap is visible from here rather than only from the region.

mod support;

use std::sync::Mutex;

use aex_control_app::ports::{
    AuthorizationReader as _, BeginWorkspaceProvisionTx, CreateApiKeyTx, CreateOrganizationTx,
    DeleteWorkspaceRequest, DeleteWorkspaceResponse, EffectError, IdempotencyRecordKey,
    ProvisionWorkspaceRequest, ProvisionWorkspaceResponse, RegionalControlPort,
};
use aex_control_app::use_cases::{CreateApiKey, CreateOrganization, CreateWorkspace};
use aex_control_aurora::{AuroraAuthorizationReader, AuroraControlStore};
use aex_control_domain::{
    AccountState, ActorKind, AuditEvent, AuditOutcome, IdempotencyKeyKind, IntentHash,
    OutboxMessage, PrincipalKindTag, ResourceKind, Scope, ScopeKind, ScopeSet, Slug, Topic,
    WorkspaceStatus,
};
use aex_identity_domain::credential::WorkspacePin;
use aex_identity_domain::{
    CredentialKind, Pepper, RegionCode, SecretRng, Verifier, mint, parse, verifier, verify,
};
use aex_wire::types::{HttpMethod, Region};
use sqlx::{Executor as _, Row as _};
use time::OffsetDateTime;
use uuid::Uuid;

/// The region every case provisions into.
const REGION: Region = Region::EuWest1;

/// A deterministic secret source.
///
/// Deterministic on purpose: a scenario that fails must fail the same way twice,
/// and the property under test is the credential's *round trip*, not its
/// entropy. The entropy of the production source is `aex-central-aws`'s own
/// evidence.
struct CountingRng(Mutex<u8>);

impl SecretRng for CountingRng {
    fn fill(&self, out: &mut [u8]) {
        let mut seed = self.0.lock().expect("the counter is not poisoned");
        for byte in out.iter_mut() {
            *seed = seed.wrapping_add(1);
            *byte = *seed;
        }
    }
}

/// The regional control plane, recorded rather than deployed.
#[derive(Default)]
struct RecordingRegion {
    provisioned: Mutex<Vec<ProvisionWorkspaceRequest>>,
}

#[async_trait::async_trait]
impl RegionalControlPort for RecordingRegion {
    async fn provision_workspace(
        &self,
        request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError> {
        self.provisioned
            .lock()
            .expect("the journal is not poisoned")
            .push(request.clone());
        Ok(ProvisionWorkspaceResponse {
            workspace_id: request.workspace_id,
            created: true,
        })
    }

    async fn delete_workspace(
        &self,
        _request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError> {
        // No case here deletes. Answering "removed" would let a future case
        // pass without the region ever being asked.
        Err(EffectError::Rejected {
            code: "scenario_does_not_delete",
            retryable: false,
        })
    }
}

/// The pepper every credential in a case is keyed under.
fn pepper() -> Pepper {
    Pepper::new([0x5a; 32])
}

/// The pepper version the seeded `control.credential_pepper` row carries.
const PEPPER_VERSION: u16 = 1;

/// The identity and control rows no production writer owns yet.
///
/// The person and the pepper are prerequisites of the ceremony rather than
/// products of it: `control.organization.created_by_user_id` is a foreign key
/// into `identity.user`, and `control.api_key.pepper_version` is a foreign key
/// into `control.credential_pepper`. Seeding anything the ceremony itself writes
/// would be the hand-seeded row this lane exists to refuse.
async fn seed(plane: &support::CentralPlane, now: OffsetDateTime) -> Uuid {
    let mut connection = plane.superuser().await;
    let user = Uuid::now_v7();
    let insert: &'static str = Box::leak(
        format!(
            "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
             VALUES ('{user}', 'founder+{}@example.test', 'active', now(), now()); \
             INSERT INTO control.credential_pepper (version, purpose, state, secret_ref, created_at) \
             VALUES ({PEPPER_VERSION}, 'api_key', 'active', 'scenario', now());",
            user.simple()
        )
        .into_boxed_str(),
    );
    connection
        .execute(insert)
        .await
        .expect("the prerequisite person and pepper are seeded");
    let _ = now;
    user
}

/// The idempotency identity one command runs under.
fn idempotency(
    key: &str,
    principal: Uuid,
    scope_kind: ScopeKind,
    scope_id: Uuid,
    route: &str,
    intent: u8,
    now: OffsetDateTime,
) -> IdempotencyRecordKey {
    IdempotencyRecordKey {
        id: Uuid::now_v7(),
        key_kind: IdempotencyKeyKind::IdempotencyKey,
        key_value: key.to_owned(),
        principal_kind: PrincipalKindTag::AccountActor,
        principal_id: principal,
        scope_kind,
        scope_id,
        method: HttpMethod::Post,
        route: route.to_owned(),
        intent_hash: IntentHash::from_bytes([intent; 32]),
        expires_at: now + time::Duration::days(1),
    }
}

/// One audit row for a command.
fn audit(
    actor: Uuid,
    organization: Option<Uuid>,
    workspace: Option<Uuid>,
    action: &str,
    resource_kind: ResourceKind,
    resource_id: Option<Uuid>,
    now: OffsetDateTime,
) -> AuditEvent {
    AuditEvent {
        id: Uuid::now_v7(),
        organization_id: organization,
        workspace_id: workspace,
        actor_kind: ActorKind::User,
        actor_id: Some(actor),
        action: action.to_owned(),
        resource_kind,
        resource_id,
        outcome: AuditOutcome::Allowed,
        request_id: "scenario-control-workspace".to_owned(),
        operation_id: None,
        detail: serde_json::json!({}),
        occurred_at: now,
    }
}

/// One outbox row for a command.
fn outbox(topic: Topic, dedupe: String, organization: Uuid, now: OffsetDateTime) -> OutboxMessage {
    OutboxMessage {
        id: Uuid::now_v7(),
        topic,
        dedupe_key: dedupe,
        group_key: organization.to_string(),
        payload: serde_json::json!({ "organizationId": organization }),
        attempts: 0,
        available_at: now,
        claimed_by: None,
        claimed_until: None,
        dispatched_at: None,
        last_error: None,
        created_at: now,
    }
}

/// Everything the scenario provisions, in one place.
struct Provisioned {
    organization: Uuid,
    workspace: Uuid,
    key_id: Uuid,
    secret: String,
}

/// Runs the whole ceremony once, against one migrated database.
#[expect(
    clippy::too_many_lines,
    reason = "one ceremony, written end to end; splitting it would hide which command precedes which"
)]
async fn provision(
    plane: &support::CentralPlane,
    region: &RecordingRegion,
    now: OffsetDateTime,
) -> Provisioned {
    let actor = seed(plane, now).await;
    let store = AuroraControlStore::new(plane.client_as("aex_control_api").await);
    let rng = CountingRng(Mutex::new(0));

    // --- the organization -------------------------------------------------
    let organization_id = Uuid::now_v7();
    let organization = CreateOrganization::run(
        &store,
        &CreateOrganizationTx {
            preassigned_id: organization_id,
            preassigned_membership_id: Uuid::now_v7(),
            name: "Acme".to_owned(),
            slug: Slug::parse("acme").expect("a valid slug"),
            created_by_user_id: actor,
            idempotency: idempotency(
                "org-1",
                actor,
                ScopeKind::Organization,
                organization_id,
                "/api/organizations",
                1,
                now,
            ),
            audit: audit(
                actor,
                Some(organization_id),
                None,
                "organization.create",
                ResourceKind::Organization,
                Some(organization_id),
                now,
            ),
            now,
        },
    )
    .await
    .expect("the organization ceremony commits");
    assert_eq!(organization.id, organization_id);

    // --- the workspace, both transactions and the effect between them ------
    let workspace_id = Uuid::now_v7();
    let provisioned = CreateWorkspace::run(
        &store,
        region,
        &BeginWorkspaceProvisionTx {
            preassigned_workspace_id: workspace_id,
            preassigned_operation_id: Uuid::now_v7(),
            organization_id,
            name: "Production".to_owned(),
            slug: Slug::parse("production").expect("a valid slug"),
            region: REGION,
            created_by_user_id: actor,
            idempotency: idempotency(
                "ws-1",
                actor,
                ScopeKind::Organization,
                organization_id,
                "/api/organizations/{organizationId}/workspaces",
                2,
                now,
            ),
            outbox: outbox(
                Topic::WorkspaceProvisionRequested,
                format!("{workspace_id}:provision"),
                organization_id,
                now,
            ),
            audit: audit(
                actor,
                Some(organization_id),
                Some(workspace_id),
                "workspace.create",
                ResourceKind::Workspace,
                Some(workspace_id),
                now,
            ),
            now,
        },
        serde_json::json!({ "resourceId": workspace_id }),
        now,
    )
    .await
    .expect("the workspace ceremony commits both transactions");
    assert_eq!(provisioned.workspace.id, workspace_id);

    // --- the API key ------------------------------------------------------
    let key_id = Uuid::now_v7();
    let (secret, digest) = mint(
        CredentialKind::WorkspaceKey,
        Some(WorkspacePin {
            region: RegionCode::new(REGION),
            workspace: workspace_id,
        }),
        key_id,
        &rng,
    );
    let created = CreateApiKey::run(
        &store,
        &CreateApiKeyTx {
            preassigned_id: key_id,
            workspace_id,
            organization_id,
            name: "scenario".to_owned(),
            scopes: ScopeSet::of(&[Scope::SessionsRead]),
            region: REGION,
            verifier: *verifier(&pepper(), &digest).as_bytes(),
            pepper_version: PEPPER_VERSION,
            created_by_user_id: actor,
            outbox: outbox(
                Topic::ApiKeyCreated,
                format!("{key_id}:created"),
                organization_id,
                now,
            ),
            idempotency: idempotency(
                "key-1",
                actor,
                ScopeKind::Workspace,
                workspace_id,
                "/api/workspaces/{workspaceId}/api-keys",
                3,
                now,
            ),
            audit: audit(
                actor,
                Some(organization_id),
                Some(workspace_id),
                "api_key.create",
                ResourceKind::ApiKey,
                Some(key_id),
                now,
            ),
            now,
        },
    )
    .await
    .expect("the API key ceremony commits");
    assert!(
        created.first,
        "the plaintext is returned only by the transaction that first committed the verifier"
    );

    Provisioned {
        organization: organization_id,
        workspace: workspace_id,
        key_id,
        secret: secret.expose().to_owned(),
    }
}

#[tokio::test]
async fn a_provisioned_workspace_key_authorizes_against_the_authority_that_issued_it() {
    let now = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("a representable instant");
    let plane = support::CentralPlane::start().await;
    let region = RecordingRegion::default();
    let provisioned = provision(&plane, &region, now).await;

    // The read side is a different login role and a different adapter: this is
    // the statement every regional request runs before it does anything else.
    let reader = AuroraAuthorizationReader::new(plane.client_as("aex_authz").await);
    let state = reader
        .resolve_workspace_key(provisioned.key_id)
        .await
        .expect("the authority answers")
        .expect("the key the ceremony minted resolves");

    assert_eq!(state.workspace_id, provisioned.workspace);
    assert_eq!(state.organization_id, provisioned.organization);
    assert_eq!(state.region, REGION);
    assert!(!state.key_revoked);
    assert_eq!(
        state.workspace_status,
        WorkspaceStatus::Active,
        "a workspace whose second transaction committed is active and publicly visible"
    );
    assert_eq!(
        state.account_state,
        AccountState::Active,
        "creating an organization calls `finance.ensure_account`, so its billing account exists \
         and is active; `unavailable` here means the finance half of the ceremony did not run"
    );
    assert_eq!(state.scopes, ScopeSet::of(&[Scope::SessionsRead]));

    // The credential round trip, through the production parser and the
    // production constant-time comparison.
    let parsed = parse(CredentialKind::WorkspaceKey, &provisioned.secret)
        .expect("the minted credential parses as a workspace key");
    assert_eq!(parsed.id, provisioned.key_id);
    assert_eq!(
        parsed.pin.map(|pin| pin.workspace),
        Some(provisioned.workspace),
        "the workspace pin in the token names the workspace the key belongs to"
    );
    assert!(
        verify(
            &pepper(),
            &parsed.digest,
            &Verifier::from_bytes(state.verifier)
        ),
        "the verifier the ceremony stored matches the secret it returned"
    );
    assert_eq!(state.pepper_version, PEPPER_VERSION);
}

#[tokio::test]
async fn the_same_idempotency_key_never_creates_a_second_organization() {
    let now = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("a representable instant");
    let plane = support::CentralPlane::start().await;
    let actor = seed(&plane, now).await;
    let store = AuroraControlStore::new(plane.client_as("aex_control_api").await);

    let first_id = Uuid::now_v7();
    let key = idempotency(
        "org-replay",
        actor,
        ScopeKind::Organization,
        first_id,
        "/api/organizations",
        7,
        now,
    );
    let command = |id: Uuid, idempotency: IdempotencyRecordKey| CreateOrganizationTx {
        preassigned_id: id,
        preassigned_membership_id: Uuid::now_v7(),
        name: "Acme".to_owned(),
        slug: Slug::parse("acme").expect("a valid slug"),
        created_by_user_id: actor,
        idempotency,
        audit: audit(
            actor,
            Some(id),
            None,
            "organization.create",
            ResourceKind::Organization,
            Some(id),
            now,
        ),
        now,
    };

    let first = CreateOrganization::run(&store, &command(first_id, key.clone()))
        .await
        .expect("the first attempt commits");
    // A retry preassigns a *different* id, exactly as a retried request would:
    // the replay record, not the caller, decides which organization exists.
    let replayed = CreateOrganization::run(
        &store,
        &command(
            Uuid::now_v7(),
            IdempotencyRecordKey {
                id: Uuid::now_v7(),
                ..key
            },
        ),
    )
    .await
    .expect("the retry replays rather than conflicting");

    assert_eq!(
        replayed.id, first.id,
        "the retry answers with the organization the first attempt created"
    );

    let mut connection = plane.superuser().await;
    let count: i64 = sqlx::query("SELECT count(*) FROM control.organization")
        .fetch_one(&mut connection)
        .await
        .expect("the organization table is readable")
        .get(0);
    assert_eq!(count, 1, "a replayed ceremony writes no second row");
}

#[tokio::test]
async fn the_central_plane_asks_the_region_to_provision_exactly_once_and_asks_for_nothing_else() {
    // The shape of this request is the whole of the central plane's contract
    // with the region. It carries an id, an organization, a region and a fence —
    // and no capacity or limits intent of any kind. A workspace with no
    // `workspace_edge_limits` row answers `401 unauthenticated` on every
    // regional request, and this assertion is where that becomes visible from
    // the central side: when the capacity bootstrap acquires a production
    // caller, this case is the one that has to change.
    let now = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("a representable instant");
    let plane = support::CentralPlane::start().await;
    let region = RecordingRegion::default();
    let provisioned = provision(&plane, &region, now).await;

    let requests = region
        .provisioned
        .lock()
        .expect("the journal is not poisoned")
        .clone();
    assert_eq!(
        requests.len(),
        1,
        "the ceremony performs exactly one regional effect"
    );
    let request = &requests[0];
    assert_eq!(request.workspace_id, provisioned.workspace);
    assert_eq!(request.organization_id, provisioned.organization);
    assert_eq!(request.region, REGION);
    assert!(
        request.fence.get() >= 1,
        "the effect carries the provisioning operation's fence"
    );

    // And the durable half the worker later dispatches.
    let mut connection = plane.superuser().await;
    let topics: Vec<String> =
        sqlx::query("SELECT topic FROM control.outbox_message ORDER BY topic")
            .fetch_all(&mut connection)
            .await
            .expect("the outbox is readable")
            .into_iter()
            .map(|row| row.get::<String, _>(0))
            .collect();
    assert_eq!(
        topics,
        vec![
            "api_key.created".to_owned(),
            "workspace.provision.requested".to_owned()
        ],
        "the ceremony enqueues exactly the two messages the worker dispatches"
    );
}
