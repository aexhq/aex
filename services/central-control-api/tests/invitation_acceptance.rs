//! `invitation_accept`, driven through the production handler and the
//! production `AcceptInvitations` use case.
//!
//! Nothing here fakes the decision. `ControlService::invitation_accept` is the
//! handler the router mounts, `AcceptInvitations::run` is the use case it calls,
//! and the only double is the store port — an in-memory transaction written to
//! the same rules as `AuroraControlStore::accept_invitations_for_email`: select
//! `pending`, unexpired rows for this exact address, create the membership or
//! **raise** an existing one, mark the invitation accepted. A test that scripted
//! the store's answer instead would prove the handler can copy a vector.
//!
//! What these cases pin is the whole reason acceptance is unusual: it names no
//! invitation. There is no token column in `control.invitation` and no
//! invitation id in the request, so the selection *is* the authorization, and
//! four of the six cases below are about what the selection must not reach.

#![allow(clippy::too_many_lines, reason = "one store double, spelled out")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_app::ports::{
    AcceptInvitationsTx, AccountProjection, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx,
    ClaimDueOperations, ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, ControlViewStore,
    CreateApiKeyTx, CreateInvitationTx, CreateOrganizationTx, DeleteWorkspaceRequest,
    DeleteWorkspaceResponse, EffectError, FinishWorkspaceProvisionTx, GcExpired, GcReport,
    ListApiKeys, ListOperations, ListOrganizations, ListWorkspaces, MembershipView, OperationView,
    OrganizationView, Page, PageRequest, ProvisionWorkspaceRequest, ProvisionWorkspaceResponse,
    RegionalControlPort, RevokeApiKeyTx, StoreError, TxOutcome, UserIdentity, WorkspaceView,
};
use aex_control_domain::{
    AccountState, ApiKey, CursorSecret, Invitation, InvitationStatus, MAX_ACCEPTABLE_INVITATIONS,
    Membership, MembershipStatus, Operation, OrgRole, Organization, OutboxMessage, Revision,
    Workspace,
};
use aex_identity_app::ports::{Clock, IdFactory, PepperKeystore, PepperPurpose};
use aex_identity_domain::credential::SecretRng;
use aex_identity_domain::{Pepper, PepperVersion};
use aex_wire::error::ErrorCode;
use aex_wire::ids::{OrganizationId, PrefixedId, UserId, Uuid7};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::models::{EmptyRequest, OrganizationRole};
use aex_wire::routes::RouteId;
use aex_wire::server::{AcceptKind, OrganizationsApi, RequestContext};
use aex_wire::types::{Region, RequestId};

use central_control_api::api::{ControlService, Store as ControlStores};

const USER: u8 = 0x51;
const OTHER_USER: u8 = 0x52;
const ORG_A: u8 = 0x31;
const ORG_B: u8 = 0x32;
const INVITE_A: u8 = 0x01;
const INVITE_B: u8 = 0x02;
const MEMBER_A: u8 = 0xAA;
const MEMBER_B: u8 = 0xBB;

const MINE: &str = "joiner@example.test";
const THEIRS: &str = "somebody-else@example.test";

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("a fixed instant")
}

/// Every identifier the public surface renders must be a `UUIDv7`, so the
/// fixtures are composed rather than counted: a `Uuid::from_u128(1)` would fail
/// at the wire boundary rather than in the assertion the case is about.
fn uuid7(tag: u8) -> Uuid7 {
    Uuid7::compose(1_767_225_600_000, [tag; 10])
}

fn raw(tag: u8) -> Uuid {
    Uuid::from_bytes(*uuid7(tag).as_bytes())
}

// ---------------------------------------------------------------------------
// The store: an in-memory transaction with the adapter's rules
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct State {
    invitations: Vec<Invitation>,
    memberships: Vec<Membership>,
    /// How many membership ids the last command preassigned.
    preassigned: Option<usize>,
}

#[derive(Debug)]
struct Store {
    state: Mutex<State>,
    identity: Option<UserIdentity>,
}

impl Store {
    fn new(identity: Option<UserIdentity>, invitations: Vec<Invitation>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                invitations,
                memberships: Vec::new(),
                preassigned: None,
            }),
            identity,
        })
    }

    fn preassigned(&self) -> Option<usize> {
        self.state.lock().expect("state").preassigned
    }

    fn with_membership(self: &Arc<Self>, membership: Membership) -> Arc<Self> {
        self.state
            .lock()
            .expect("state")
            .memberships
            .push(membership);
        Arc::clone(self)
    }

    fn invitation(&self, id: Uuid) -> Invitation {
        self.state
            .lock()
            .expect("state")
            .invitations
            .iter()
            .find(|invitation| invitation.id == id)
            .cloned()
            .expect("the invitation is still there")
    }

    fn memberships(&self) -> Vec<Membership> {
        self.state.lock().expect("state").memberships.clone()
    }
}

const UNDRIVEN: &str = "this suite drives invitation acceptance only";

#[async_trait]
impl ControlStore for Store {
    async fn accept_invitations_for_email(
        &self,
        command: &AcceptInvitationsTx,
    ) -> Result<TxOutcome<Vec<Membership>>, StoreError> {
        // The adapter refuses before it reads; so does this.
        if !command.email_verified {
            return Ok(TxOutcome::Replayed(Vec::new()));
        }
        let mut state = self.state.lock().expect("state");
        state.preassigned = Some(command.preassigned_membership_ids.len());

        // `FIND_ACCEPTABLE_INVITATIONS`: this exact address, pending, unexpired,
        // oldest first, bounded.
        let acceptable: Vec<Invitation> = state
            .invitations
            .iter()
            .filter(|invitation| {
                invitation.email == command.email
                    && invitation.status == InvitationStatus::Pending
                    && invitation.expires_at > command.now
            })
            .take(MAX_ACCEPTABLE_INVITATIONS)
            .cloned()
            .collect();
        if acceptable.is_empty() {
            return Ok(TxOutcome::Replayed(Vec::new()));
        }
        if command.preassigned_membership_ids.len() < acceptable.len() {
            return Err(StoreError::Fatal(
                "invitation acceptance needs at least one preassigned membership id per row"
                    .to_owned(),
            ));
        }

        let mut produced = Vec::with_capacity(acceptable.len());
        for (invitation, membership_id) in acceptable
            .iter()
            .zip(command.preassigned_membership_ids.iter().copied())
        {
            // `membership_for_invitation`: raise an existing membership, never
            // lower it; otherwise insert one at the invited role.
            let existing = state.memberships.iter_mut().find(|membership| {
                membership.organization_id == invitation.organization_id
                    && membership.user_id == command.user_id
                    && membership.status == MembershipStatus::Active
            });
            let membership = if let Some(existing) = existing {
                *existing = existing.raise_role_to(invitation.role, command.now);
                existing.clone()
            } else {
                let fresh = Membership {
                    id: membership_id,
                    organization_id: invitation.organization_id,
                    user_id: command.user_id,
                    role: invitation.role,
                    status: MembershipStatus::Active,
                    revision: Revision::INITIAL,
                    created_at: command.now,
                    updated_at: command.now,
                };
                state.memberships.push(fresh.clone());
                fresh
            };

            // `ACCEPT_INVITATION`.
            let row = state
                .invitations
                .iter_mut()
                .find(|row| row.id == invitation.id)
                .expect("the selected row");
            *row = row
                .accept(command.user_id, &command.email, true, command.now)
                .map_err(|error| StoreError::Fatal(error.to_string()))?;
            produced.push(membership);
        }
        Ok(TxOutcome::Committed(produced))
    }

    async fn create_organization(
        &self,
        _command: &CreateOrganizationTx,
    ) -> Result<TxOutcome<Organization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_organizations(
        &self,
        _query: &ListOrganizations,
    ) -> Result<Page<Organization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_organization(&self, _id: Uuid) -> Result<Option<Organization>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_memberships(
        &self,
        _organization_id: Uuid,
        _page: &PageRequest,
    ) -> Result<Page<Membership>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn create_invitation(
        &self,
        _command: &CreateInvitationTx,
    ) -> Result<TxOutcome<Invitation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn begin_workspace_provision(
        &self,
        _command: &BeginWorkspaceProvisionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn finish_workspace_provision(
        &self,
        _command: &FinishWorkspaceProvisionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_workspaces(
        &self,
        _query: &ListWorkspaces,
    ) -> Result<Page<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_workspace(&self, _id: Uuid) -> Result<Option<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn begin_workspace_deletion(
        &self,
        _command: &BeginWorkspaceDeletionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn complete_workspace_deletion(
        &self,
        _command: &CompleteWorkspaceDeletionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn create_api_key(
        &self,
        _command: &CreateApiKeyTx,
    ) -> Result<TxOutcome<ApiKey>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_api_key(&self, _id: Uuid) -> Result<Option<ApiKey>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_api_keys(&self, _query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn revoke_api_key(&self, _command: &RevokeApiKeyTx) -> Result<TxOutcome<()>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_operation(&self, _id: Uuid) -> Result<Option<Operation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_operations(
        &self,
        _query: &ListOperations,
    ) -> Result<Page<Operation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn claim_due_operations(
        &self,
        _command: &ClaimDueOperations,
    ) -> Result<Vec<Operation>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn claim_outbox(&self, _command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn enqueue_outbox(&self, _message: &OutboxMessage) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn mark_outbox_dispatched(
        &self,
        _id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn release_outbox(
        &self,
        _id: Uuid,
        _available_at: OffsetDateTime,
        _error: &str,
    ) -> Result<(), StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn gc_expired(&self, _command: &GcExpired) -> Result<GcReport, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, StoreError> {
        unreachable!("{UNDRIVEN}")
    }
}

#[async_trait]
impl ControlViewStore for Store {
    async fn user_identity(&self, _user_id: Uuid) -> Result<Option<UserIdentity>, StoreError> {
        Ok(self.identity.clone())
    }

    async fn list_organization_views(
        &self,
        _query: &ListOrganizations,
    ) -> Result<Page<OrganizationView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_organization_view(
        &self,
        _organization_id: Uuid,
        _user_id: Uuid,
    ) -> Result<Option<OrganizationView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_membership_views(
        &self,
        _organization_id: Uuid,
        _page: &PageRequest,
    ) -> Result<Page<MembershipView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_workspace_views(
        &self,
        _query: &ListWorkspaces,
    ) -> Result<Page<WorkspaceView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_workspace_view(&self, _id: Uuid) -> Result<Option<WorkspaceView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn get_operation_view(&self, _id: Uuid) -> Result<Option<OperationView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn list_operation_views(
        &self,
        _query: &ListOperations,
    ) -> Result<Page<OperationView>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn account_profile(
        &self,
        _organization_id: Uuid,
    ) -> Result<Option<AccountProjection>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }

    async fn idempotency_id_for_operation(
        &self,
        _operation_id: Uuid,
    ) -> Result<Option<Uuid>, StoreError> {
        unreachable!("{UNDRIVEN}")
    }
}

// ---------------------------------------------------------------------------
// The ambient collaborators the composition root supplies
// ---------------------------------------------------------------------------

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        at()
    }
}

/// Time-ordered ids, distinct per call, so a preassignment can be told apart
/// from a row that already existed.
struct CountingIds(Mutex<u128>);

impl IdFactory for CountingIds {
    fn next(&self) -> Uuid {
        let mut counter = self.0.lock().expect("counter");
        *counter += 1;
        Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0000 + *counter)
    }
}

struct ZeroRng;

impl SecretRng for ZeroRng {
    fn fill(&self, out: &mut [u8]) {
        out.fill(0);
    }
}

struct NoPeppers;

#[async_trait]
impl PepperKeystore for NoPeppers {
    async fn active(&self, _purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError> {
        unreachable!("acceptance mints no credential")
    }

    async fn by_version(
        &self,
        _purpose: PepperPurpose,
        _version: PepperVersion,
    ) -> Result<Pepper, StoreError> {
        unreachable!("acceptance verifies no credential")
    }
}

struct NoRegion;

#[async_trait]
impl RegionalControlPort for NoRegion {
    async fn provision_workspace(
        &self,
        _request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError> {
        unreachable!("acceptance never leaves the central plane")
    }

    async fn delete_workspace(
        &self,
        _request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError> {
        unreachable!("acceptance never leaves the central plane")
    }
}

fn service(store: &Arc<Store>) -> ControlService {
    ControlService::new(
        Arc::clone(store) as Arc<dyn ControlStores>,
        Arc::new(NoPeppers),
        Arc::new(NoRegion),
        Arc::new(FixedClock),
        Arc::new(CountingIds(Mutex::new(0))),
        Arc::new(ZeroRng),
        Arc::new(CursorSecret::new([3_u8; 32])),
        Region::EuWest1,
        BTreeMap::new(),
    )
}

fn context() -> RequestContext {
    RequestContext {
        request_id: RequestId::parse("req-accept").expect("a request id"),
        route: RouteId::InvitationAccept,
        // An account token, not a browser session: the acceptance ceremony
        // needs only *who* the caller is, so there is no session to name.
        actor_session_id: None,
        principal: PrincipalScope::Account {
            user: UserId::from_uuid7(uuid7(USER)),
            organization: None,
        },
        granted_scopes: aex_wire::scopes::ScopeSet::empty(),
        idempotency_key: None,
        operation_id: None,
        if_match: None,
        accept: AcceptKind::Json,
    }
}

fn invitation(id: u8, organization: u8, email: &str, role: OrgRole) -> Invitation {
    Invitation {
        id: raw(id),
        organization_id: raw(organization),
        email: email.to_owned(),
        role,
        status: InvitationStatus::Pending,
        invited_by_user_id: raw(OTHER_USER),
        accepted_user_id: None,
        created_at: at() - Duration::days(1),
        expires_at: at() + Duration::days(13),
        resolved_at: None,
    }
}

fn verified() -> UserIdentity {
    identity(true)
}

fn identity(email_verified: bool) -> UserIdentity {
    UserIdentity {
        email: MINE.to_owned(),
        email_verified,
    }
}

fn organization_of(id: u8) -> OrganizationId {
    OrganizationId::from_uuid7(uuid7(id))
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_redeems_every_pending_invitation_for_the_verified_address() {
    // Acceptance names no invitation, so "accept" means "accept all of mine".
    // Two organizations inviting the same person is one request, not two.
    let store = Store::new(
        Some(verified()),
        vec![
            invitation(INVITE_A, ORG_A, MINE, OrgRole::Admin),
            invitation(INVITE_B, ORG_B, MINE, OrgRole::Member),
        ],
    );
    let result = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("a verified address may accept");

    assert_eq!(result.memberships.len(), 2);
    let roles: Vec<(OrganizationId, OrganizationRole)> = result
        .memberships
        .iter()
        .map(|membership| (membership.organization_id, membership.role))
        .collect();
    assert!(roles.contains(&(organization_of(ORG_A), OrganizationRole::Admin)));
    assert!(roles.contains(&(organization_of(ORG_B), OrganizationRole::Member)));
    // How many invitations are acceptable is only knowable under the
    // transaction's own lock, so the handler cannot count first without racing
    // itself: it supplies the selection's ceiling and a prefix is consumed.
    assert_eq!(store.preassigned(), Some(MAX_ACCEPTABLE_INVITATIONS));
    for membership in &result.memberships {
        assert_eq!(membership.email, MINE, "the membership names the acceptor");
    }
    for id in [0x01, 0x02] {
        let row = store.invitation(raw(id));
        assert_eq!(row.status, InvitationStatus::Accepted);
        assert_eq!(row.accepted_user_id, Some(raw(USER)));
    }
}

#[tokio::test]
async fn a_repeated_call_succeeds_and_redeems_nothing() {
    // The route carries no replay identity because it needs none: the selection
    // reads `pending`, so the second call has nothing to select. It must be a
    // `200` with an empty list, not a conflict — otherwise a retried request
    // that already succeeded would report failure.
    let store = Store::new(
        Some(verified()),
        vec![invitation(INVITE_A, ORG_A, MINE, OrgRole::Member)],
    );
    let service = service(&store);
    let first = service
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("the first call redeems");
    assert_eq!(first.memberships.len(), 1);

    let second = service
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("the second call is still a success");
    assert!(second.memberships.is_empty());
    assert_eq!(store.memberships().len(), 1, "no duplicate membership");
}

#[tokio::test]
async fn an_unverified_address_is_refused_rather_than_answered_empty() {
    // The use case answers `Ok(vec![])` for an unverified address, which is the
    // right shape for a command and the wrong answer for a caller: it is
    // indistinguishable from "you have no invitations". The handler refuses.
    let store = Store::new(
        Some(identity(false)),
        vec![invitation(INVITE_A, ORG_A, MINE, OrgRole::Member)],
    );
    let error = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect_err("an unverified address cannot accept");
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(
        store.invitation(raw(INVITE_A)).status,
        InvitationStatus::Pending,
        "a refused acceptance resolves nothing"
    );
    assert!(store.memberships().is_empty());
    assert_eq!(
        store.preassigned(),
        None,
        "the refusal is decided before the transaction is opened"
    );
}

#[tokio::test]
async fn a_principal_with_no_identity_row_is_refused() {
    // A credential resolved to a user that `identity.user` does not hold is not
    // an empty acceptance, it is an incoherent request. There is no address to
    // select on, so there is nothing this route could honestly do.
    let store = Store::new(None, vec![invitation(INVITE_A, ORG_A, MINE, OrgRole::Member)]);
    let error = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect_err("a principal with no identity row cannot accept");
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(store.preassigned(), None);
}

#[tokio::test]
async fn an_invitation_addressed_to_somebody_else_is_never_selected() {
    // This is the tenant boundary. There is no role floor on this route and
    // there cannot be one, so the only thing keeping the caller out of another
    // organization is that the selection matches on their own address.
    let store = Store::new(
        Some(verified()),
        vec![invitation(INVITE_A, ORG_A, THEIRS, OrgRole::Admin)],
    );
    let result = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("the call succeeds and selects nothing");
    assert!(result.memberships.is_empty());
    assert_eq!(
        store.invitation(raw(INVITE_A)).status,
        InvitationStatus::Pending
    );
    assert!(store.memberships().is_empty(), "no membership anywhere");
}

#[tokio::test]
async fn an_expired_invitation_is_not_redeemed() {
    let mut lapsed = invitation(INVITE_A, ORG_A, MINE, OrgRole::Admin);
    lapsed.created_at = at() - Duration::days(30);
    lapsed.expires_at = at() - Duration::seconds(1);
    let store = Store::new(Some(verified()), vec![lapsed]);

    let result = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("the call succeeds and selects nothing");
    assert!(result.memberships.is_empty());
    assert_eq!(
        store.invitation(raw(INVITE_A)).status,
        InvitationStatus::Pending,
        "expiry is a selection predicate, not a write"
    );
}

#[tokio::test]
async fn accepting_raises_an_existing_membership_and_never_lowers_one() {
    // An invitation may re-reach somebody who is already inside. The domain
    // raises, so a `member` invited as `admin` is promoted; it never lowers, so
    // an `owner` invited as `member` keeps ownership. `check_offered_role`
    // already forbids inviting `owner`, so acceptance can never mint one.
    let store = Store::new(
        Some(verified()),
        vec![
            invitation(INVITE_A, ORG_A, MINE, OrgRole::Admin),
            invitation(INVITE_B, ORG_B, MINE, OrgRole::Member),
        ],
    )
    .with_membership(Membership {
        id: raw(MEMBER_A),
        organization_id: raw(ORG_A),
        user_id: raw(USER),
        role: OrgRole::Member,
        status: MembershipStatus::Active,
        revision: Revision::INITIAL,
        created_at: at() - Duration::days(2),
        updated_at: at() - Duration::days(2),
    })
    .with_membership(Membership {
        id: raw(MEMBER_B),
        organization_id: raw(ORG_B),
        user_id: raw(USER),
        role: OrgRole::Owner,
        status: MembershipStatus::Active,
        revision: Revision::INITIAL,
        created_at: at() - Duration::days(2),
        updated_at: at() - Duration::days(2),
    });

    let result = service(&store)
        .invitation_accept(&context(), EmptyRequest {})
        .await
        .expect("a verified address may accept");
    assert_eq!(result.memberships.len(), 2);

    let memberships = store.memberships();
    assert_eq!(memberships.len(), 2, "no membership was duplicated");
    let raised = memberships
        .iter()
        .find(|membership| membership.organization_id == raw(ORG_A))
        .expect("the raised membership");
    assert_eq!(raised.id, raw(MEMBER_A), "the same row was raised");
    assert_eq!(raised.role, OrgRole::Admin);

    let untouched = memberships
        .iter()
        .find(|membership| membership.organization_id == raw(ORG_B))
        .expect("the owner membership");
    assert_eq!(
        untouched.role,
        OrgRole::Owner,
        "an invitation may raise a role and may never lower one"
    );
}

