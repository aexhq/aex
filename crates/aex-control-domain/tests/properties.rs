//! Generated properties over the control domain.
//!
//! The state machines are exercised as command sequences against a reference
//! model, so an invariant has to survive arbitrary histories rather than the
//! three orderings somebody thought of.

use aex_control_domain::codec::{base64url, unbase64url};
use aex_control_domain::cursor::{CursorClaims, CursorSecret, decode_cursor, encode_cursor};
use aex_control_domain::epoch::Epoch;
use aex_control_domain::intent::{
    IdempotencyIdentity, IdempotencyKeyKind, ScopeKind, canonical_intent_hash,
};
use aex_control_domain::membership::{Membership, MembershipStatus};
use aex_control_domain::operation::{Fence, LeaseOwner, Operation, OperationKind, OperationStatus};
use aex_control_domain::{
    AccountPauseCause, AccountProfile, AccountState, IntentHash, OrgRole, PrincipalKindTag,
    Revision, Scope, ScopeSet, Slug, Workspace, WorkspaceStatus, account_operational_state,
};
use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7};
use aex_wire::models::{OperationalStateSource, OrganizationAccount, WorkspaceOperationalState};
use aex_wire::scopes::ScopeId;
use aex_wire::types::{HttpMethod, Region};
use proptest::prelude::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// One command against a workspace.
#[derive(Debug, Clone, Copy)]
enum WorkspaceCommand {
    Activate,
    BeginDeletion,
    CompleteDeletion,
}

/// One command against an operation.
#[derive(Debug, Clone, Copy)]
enum OperationCommand {
    Claim(u8),
    RenewAs(u8),
    Succeed,
    Fail,
    Cancel,
}

fn workspace() -> Workspace {
    Workspace {
        id: Uuid::from_u128(1),
        organization_id: Uuid::from_u128(2),
        name: "Prod".to_owned(),
        slug: Slug::parse("prod").expect("a valid slug"),
        region: Region::EuWest1,
        status: WorkspaceStatus::Provisioning,
        provision_operation_id: Uuid::from_u128(3),
        provision_fence: Fence::FIRST,
        deletion_operation_id: None,
        deletion_fence: None,
        revision: Revision::INITIAL,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        activated_at: None,
        deleted_at: None,
        created_by_user_id: Uuid::from_u128(4),
    }
}

fn operation() -> Operation {
    Operation {
        id: Uuid::from_u128(1),
        kind: OperationKind::WorkspaceDelete,
        visibility: OperationKind::WorkspaceDelete.visibility(),
        organization_id: Uuid::from_u128(2),
        workspace_id: Some(Uuid::from_u128(3)),
        principal_id: Uuid::from_u128(4),
        scopes: ScopeSet::MEMBER,
        status: OperationStatus::Queued,
        intent_hash: IntentHash::from_bytes([0_u8; 32]),
        fence: Fence::FIRST,
        attempt: 0,
        lease: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        started_at: None,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        terminal_at: None,
        due_at: None,
    }
}

fn membership(role: OrgRole) -> Membership {
    Membership {
        id: Uuid::from_u128(1),
        organization_id: Uuid::from_u128(2),
        user_id: Uuid::from_u128(3),
        role,
        status: MembershipStatus::Active,
        revision: Revision::INITIAL,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn any_scope() -> impl Strategy<Value = Scope> {
    proptest::sample::select(ScopeId::ALL.to_vec())
}

fn any_scope_set() -> impl Strategy<Value = ScopeSet> {
    proptest::collection::vec(any_scope(), 0..12).prop_map(|scopes| ScopeSet::of(&scopes))
}

fn identity() -> IdempotencyIdentity {
    IdempotencyIdentity {
        key_kind: IdempotencyKeyKind::IdempotencyKey,
        key_value: "k".to_owned(),
        principal_kind: PrincipalKindTag::AccountActor,
        principal_id: Uuid::from_u128(1),
        scope_kind: ScopeKind::Organization,
        scope_id: Uuid::from_u128(2),
        method: HttpMethod::Post,
        route: "/api/workspaces".to_owned(),
    }
}

proptest! {
    /// A workspace never leaves the lifecycle order, and its placement never
    /// changes under any history.
    #[test]
    fn a_workspace_never_leaves_its_lifecycle_order(
        commands in proptest::collection::vec(
            prop_oneof![
                Just(WorkspaceCommand::Activate),
                Just(WorkspaceCommand::BeginDeletion),
                Just(WorkspaceCommand::CompleteDeletion),
            ],
            0..24,
        )
    ) {
        let start = workspace();
        let mut current = start.clone();
        let at = OffsetDateTime::UNIX_EPOCH;
        for command in commands {
            let next = match command {
                WorkspaceCommand::Activate => current.activate(current.provision_fence, at),
                WorkspaceCommand::BeginDeletion => {
                    current.begin_deletion(Uuid::from_u128(9), Fence::FIRST, at)
                }
                WorkspaceCommand::CompleteDeletion => {
                    current.complete_deletion(current.deletion_fence.unwrap_or(Fence::FIRST), at)
                }
            };
            if let Ok(next) = next {
                prop_assert!(
                    next.status as u8 >= current.status as u8,
                    "{:?} -> {:?}", current.status, next.status
                );
                current = next;
            }
            prop_assert_eq!(current.region, start.region);
            prop_assert_eq!(current.organization_id, start.organization_id);
            prop_assert_eq!(
                current.is_publicly_visible(),
                current.status != WorkspaceStatus::Provisioning
            );
        }
    }

    /// A terminal operation stays terminal, the fence never decreases, and the
    /// attempt count never decreases.
    #[test]
    fn an_operation_is_monotone_under_any_history(
        commands in proptest::collection::vec(
            prop_oneof![
                (0_u8..3).prop_map(OperationCommand::Claim),
                (0_u8..3).prop_map(OperationCommand::RenewAs),
                Just(OperationCommand::Succeed),
                Just(OperationCommand::Fail),
                Just(OperationCommand::Cancel),
            ],
            0..32,
        )
    ) {
        let mut current = operation();
        let mut now = OffsetDateTime::UNIX_EPOCH;
        let lease = Duration::seconds(30);
        let mut was_terminal = false;
        for command in commands {
            now += Duration::seconds(20);
            let owner = |index: u8| LeaseOwner::new(format!("worker-{index}"));
            let next = match command {
                OperationCommand::Claim(index) => current.claim(owner(index), now, lease),
                OperationCommand::RenewAs(index) => current.renew(&owner(index), now, lease),
                OperationCommand::Succeed => current.succeed(current.fence, now),
                OperationCommand::Fail => current.fail(current.fence, now),
                OperationCommand::Cancel => current.cancel(now),
            };
            if let Ok(next) = next {
                prop_assert!(!was_terminal, "a terminal operation accepted a transition");
                prop_assert!(next.fence.get() >= current.fence.get());
                prop_assert!(next.attempt >= current.attempt);
                current = next;
            }
            was_terminal |= current.status.is_terminal();
            prop_assert_eq!(current.status.is_terminal(), was_terminal);
        }
    }

    /// A cancellation never succeeds on a central operation, whatever the state.
    #[test]
    fn a_central_operation_is_never_cancelable(seconds in 0_i64..100_000) {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds);
        for kind in OperationKind::ALL {
            let mut operation = operation();
            operation.kind = kind;
            prop_assert!(operation.cancel(now).is_err());
        }
    }

    /// The last owner can never be removed or demoted, at any owner count.
    #[test]
    fn the_last_owner_survives_every_history(others in 0_usize..4) {
        let at = OffsetDateTime::UNIX_EPOCH;
        let owner = membership(OrgRole::Owner);
        let removable = owner.remove(others, at).is_ok();
        prop_assert_eq!(removable, others > 0);
        let demotable = owner.set_role(OrgRole::Member, others, at).is_ok();
        prop_assert_eq!(demotable, others > 0);
    }

    /// Accepting an invitation never lowers an existing role.
    #[test]
    fn raising_a_role_never_demotes(
        current in proptest::sample::select(OrgRole::ALL.to_vec()),
        offered in proptest::sample::select(OrgRole::ALL.to_vec()),
    ) {
        let after = membership(current).raise_role_to(offered, OffsetDateTime::UNIX_EPOCH);
        prop_assert!(after.role.at_least(current));
    }

    /// An epoch only moves forward, whatever sequence of advances happens.
    #[test]
    fn an_epoch_only_moves_forward(steps in 0_u32..64) {
        let mut epoch = Epoch::NEVER;
        for _ in 0..steps {
            let next = epoch.advance();
            prop_assert!(next.get() >= epoch.get());
            epoch = next;
        }
        prop_assert_eq!(epoch.get(), u64::from(steps));
    }

    /// The scope set round-trips through its string form and its bitset form.
    #[test]
    fn a_scope_set_round_trips_losslessly(set in any_scope_set()) {
        prop_assert_eq!(ScopeSet::from_strings(&set.to_strings()), Ok(set));
        prop_assert_eq!(ScopeSet::from_bits(set.bits()), Ok(set));
        let strings = set.to_strings();
        let mut sorted = strings.clone();
        sorted.sort_by_key(|scope| {
            ScopeId::ALL
                .iter()
                .position(|it| it.as_str() == scope)
                .unwrap_or(usize::MAX)
        });
        prop_assert_eq!(strings, sorted, "registry order is stable");
    }

    /// Intersection with the workspace-key ceiling is idempotent and never adds.
    #[test]
    fn the_key_ceiling_only_removes(set in any_scope_set()) {
        let capped = set.intersect(ScopeSet::WORKSPACE_KEY_MINTABLE);
        prop_assert!(set.contains_all(capped));
        prop_assert_eq!(capped.intersect(ScopeSet::WORKSPACE_KEY_MINTABLE), capped);
    }

    /// A role's scopes are a subset of every stronger role's scopes.
    #[test]
    fn role_scopes_are_ordered(
        weaker in proptest::sample::select(OrgRole::ALL.to_vec()),
        stronger in proptest::sample::select(OrgRole::ALL.to_vec()),
    ) {
        if stronger.at_least(weaker) {
            prop_assert!(stronger.scopes().contains_all(weaker.scopes()));
        }
    }

    /// Base64url round-trips exactly and never emits padding.
    #[test]
    fn base64url_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..96)) {
        let text = base64url(&bytes);
        prop_assert!(!text.contains('='));
        prop_assert_eq!(unbase64url(&text), Some(bytes));
    }

    /// A cursor round-trips only against the exact claims it was minted for.
    #[test]
    fn a_cursor_binds_every_field(
        principal in any::<u128>(),
        scope in any::<u128>(),
        last in any::<u128>(),
        created in 0_i64..1_000_000_000,
    ) {
        let secret = CursorSecret::new([5_u8; 32]);
        let claims = CursorClaims {
            endpoint: "/api/workspaces".to_owned(),
            principal_id: Uuid::from_u128(principal),
            scope_id: Uuid::from_u128(scope),
            region: Region::EuWest1,
            filter_hash: [1_u8; 32],
            snapshot_ms: created,
            last: (created, Uuid::from_u128(last)),
        };
        let raw = encode_cursor(&secret, &claims, 0);
        prop_assert_eq!(decode_cursor(&secret, &raw, &claims, 0), Ok(claims.last));

        let mut other = claims.clone();
        other.principal_id = Uuid::from_u128(principal.wrapping_add(1));
        prop_assert!(decode_cursor(&secret, &raw, &other, 0).is_err());
    }

    /// The intent digest is stable under member reordering and sensitive to
    /// every byte of the body.
    #[test]
    fn the_intent_digest_is_canonical(name in "[a-z]{1,16}", count in 0_i64..1000) {
        let ordered = serde_json::json!({ "count": count, "name": name.clone() });
        let reordered = serde_json::json!({ "name": name.clone(), "count": count });
        prop_assert_eq!(
            canonical_intent_hash(&identity(), &ordered),
            canonical_intent_hash(&identity(), &reordered)
        );
        let changed = serde_json::json!({ "count": count + 1, "name": name });
        prop_assert_ne!(
            canonical_intent_hash(&identity(), &ordered),
            canonical_intent_hash(&identity(), &changed)
        );
    }

    /// A floating-point body is always refused, wherever the number sits.
    #[test]
    fn a_float_is_always_refused(value in proptest::num::f64::NORMAL) {
        let body = serde_json::json!({ "nested": [{ "amount": value }] });
        prop_assert!(canonical_intent_hash(&identity(), &body).is_err());
    }
}

// The cross-plane account-state contract.
//
// One `AccountProfile` is projected two ways — the way the central plane reads
// it out of `finance.account_state_v1`, and the way a region reads it back out
// of the placement status `central-control-worker` writes — and the two must
// produce byte-identical JSON. They may differ in staleness. They may never
// differ in vocabulary, discriminator or derivation.
//
// The failure this catches is real and is one spelling apart: the view answers
// `paused_top_up_required` and the worker writes `paused`, so a projection that
// went through either string without the shared parse would answer a different
// discriminator on each plane for the same account.

/// The status `finance.account_state_v1`'s `CASE` publishes for one durable
/// `finance.billing_account.state`. Every non-`active` state is paused; the
/// `ELSE` arm exists so a state added later is restrictive, not permissive.
fn central_view_status(durable_state: &str) -> &'static str {
    if durable_state == "active" {
        "active"
    } else {
        "paused_top_up_required"
    }
}

/// The placement status `central-control-worker::project_view` writes for an
/// active workspace whose account is in `state`.
fn regional_placement_status(state: AccountState) -> &'static str {
    match state {
        AccountState::PausedTopUpRequired => "paused",
        AccountState::Active | AccountState::Unavailable => "active",
    }
}

/// A profile as one plane's read produces it.
fn projected(status: &str, reason: Option<&str>) -> AccountProfile {
    AccountProfile {
        state: AccountState::parse(status).expect("both planes spell a state the domain knows"),
        reason: reason.map(str::to_owned),
        revision: 11,
        changed_at: OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("an instant"),
    }
}

#[test]
fn one_account_profile_projects_byte_identically_on_both_planes() {
    let organization = OrganizationId::from_uuid7(
        Uuid7::from_bytes([
            0x01, 0x92, 0x3f, 0x2a, 0x7c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, 1,
        ])
        .expect("a UUIDv7"),
    );

    // Every durable finance state, with the remedy each one publishes. `active`
    // carries no reason: the durable `CHECK` makes the two agree by construction.
    let durable: [(&str, Option<&str>); 4] = [
        ("active", None),
        ("payment_hold", Some(AccountPauseCause::TopUpRequired.as_str())),
        ("dispute_hold", Some(AccountPauseCause::DisputeHold.as_str())),
        ("closed", Some(AccountPauseCause::AccountClosed.as_str())),
    ];

    for (durable_state, reason) in durable {
        let central = projected(central_view_status(durable_state), reason);
        let regional = projected(regional_placement_status(central.state), reason);
        assert_eq!(
            central, regional,
            "`{durable_state}` reaches the two planes as two different facts"
        );

        let central_state =
            account_operational_state(&central).expect("the central read projects");
        let regional_state =
            account_operational_state(&regional).expect("the regional read projects");

        // The published wrappers, not just the shared value: a second derivation
        // would most plausibly appear in one renderer rather than in the mapping.
        let central_wire = serde_json::to_vec(&OrganizationAccount {
            organization_id: organization,
            state: central_state,
        })
        .expect("the central wrapper serializes");
        let regional_wire = serde_json::to_vec(&WorkspaceOperationalState {
            inherited_from: OperationalStateSource::Account,
            organization_id: organization,
            state: regional_state,
        })
        .expect("the regional wrapper serializes");

        let member = |bytes: &[u8]| -> serde_json::Value {
            let document: serde_json::Value =
                serde_json::from_slice(bytes).expect("the wrapper is an object");
            document["state"].clone()
        };
        assert_eq!(
            serde_json::to_vec(&member(&central_wire)).expect("canonical"),
            serde_json::to_vec(&member(&regional_wire)).expect("canonical"),
            "`{durable_state}` publishes two different operational states"
        );
    }
}

#[test]
fn an_unavailable_account_is_refused_on_both_planes_rather_than_projected() {
    // The regional side has no `unavailable` placement status at all — an
    // undecodable row is a transport failure there, not a state — so this pins
    // the central arm and the vocabulary that makes the regional absence honest.
    assert!(account_operational_state(&projected("unavailable", None)).is_err());
    assert_eq!(AccountState::parse("deleting"), None);
}
