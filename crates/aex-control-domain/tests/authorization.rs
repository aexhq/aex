//! The central authorization matrix, cell by cell.
//!
//! Every row of the matrix is exercised in both directions — the principal that
//! may and the principal that may not — plus the two invariants that close
//! standing defects: a workspace key can never be redirected at another
//! organization or workspace, and no route reaches a handler without either an
//! organization or a pause exemption.

use aex_control_domain::{
    AccountState, Action, ActorCredential, Admission, Denial, OrgMembership, OrgRole, Principal,
    PrincipalKindTag, Resource, ResourceClass, ScopeSet, admit, decide, requirement,
};
use aex_wire::routes::{RouteId, route};
use aex_wire::scopes::ScopeId;
use uuid::Uuid;

const ORG: u128 = 0x11;
const OTHER_ORG: u128 = 0x12;
const WORKSPACE: u128 = 0x21;
const OTHER_WORKSPACE: u128 = 0x22;
const KEY: u128 = 0x31;
const OPERATION: u128 = 0x41;
const USER: u128 = 0x51;

fn actor(role: OrgRole, scopes: ScopeSet) -> Principal {
    Principal::AccountActor {
        user_id: Uuid::from_u128(USER),
        credential: ActorCredential::AccountToken(Uuid::from_u128(0x61)),
        memberships: vec![OrgMembership {
            organization_id: Uuid::from_u128(ORG),
            membership_id: Uuid::from_u128(0x71),
            role,
        }],
        token_scopes: scopes,
    }
}

fn session_actor(role: OrgRole) -> Principal {
    Principal::AccountActor {
        user_id: Uuid::from_u128(USER),
        credential: ActorCredential::DashboardSession(Uuid::from_u128(0x62)),
        memberships: vec![OrgMembership {
            organization_id: Uuid::from_u128(ORG),
            membership_id: Uuid::from_u128(0x71),
            role,
        }],
        token_scopes: ScopeSet::ALL,
    }
}

fn key(scopes: ScopeSet) -> Principal {
    Principal::WorkspaceKey {
        key_id: Uuid::from_u128(KEY),
        workspace_id: Uuid::from_u128(WORKSPACE),
        organization_id: Uuid::from_u128(ORG),
        scopes,
    }
}

fn action(route: RouteId) -> Action {
    Action::central(route).expect("a central route")
}

fn organization() -> Resource {
    Resource::Organization(Uuid::from_u128(ORG))
}

fn workspace() -> Resource {
    Resource::Workspace {
        workspace_id: Uuid::from_u128(WORKSPACE),
        organization_id: Uuid::from_u128(ORG),
    }
}

fn operation() -> Resource {
    Resource::Operation {
        operation_id: Uuid::from_u128(OPERATION),
        organization_id: Uuid::from_u128(ORG),
    }
}

fn api_key() -> Resource {
    Resource::ApiKey {
        key_id: Uuid::from_u128(KEY),
        workspace_id: Uuid::from_u128(WORKSPACE),
        organization_id: Uuid::from_u128(ORG),
    }
}

/// The resource a route's class expects, filled with the fixture ids.
fn resource_for(class: ResourceClass) -> Resource {
    match class {
        ResourceClass::None => Resource::None,
        ResourceClass::Organization => organization(),
        ResourceClass::Workspace => workspace(),
        ResourceClass::ApiKey => api_key(),
        ResourceClass::Operation => operation(),
    }
}

#[test]
fn the_role_floor_holds_in_both_directions() {
    let table: &[(RouteId, ResourceClass, OrgRole)] = &[
        (
            RouteId::OrganizationGet,
            ResourceClass::Organization,
            OrgRole::Member,
        ),
        (
            RouteId::MembershipsList,
            ResourceClass::Organization,
            OrgRole::Member,
        ),
        (
            RouteId::InvitationCreate,
            ResourceClass::Organization,
            OrgRole::Admin,
        ),
        (
            RouteId::WorkspaceGet,
            ResourceClass::Workspace,
            OrgRole::Member,
        ),
        (
            RouteId::WorkspaceDelete,
            ResourceClass::Workspace,
            OrgRole::Owner,
        ),
        (RouteId::ApiKeyRevoke, ResourceClass::ApiKey, OrgRole::Admin),
        (
            RouteId::CentralOperationGet,
            ResourceClass::Operation,
            OrgRole::Member,
        ),
        (
            RouteId::CentralOperationCancel,
            ResourceClass::Operation,
            OrgRole::Member,
        ),
    ];

    for (route, class, floor) in table {
        let action = action(*route);
        assert_eq!(requirement(action).resource_class, *class, "{route:?}");
        let resource = resource_for(*class);
        for role in OrgRole::ALL {
            let outcome = decide(&actor(role, ScopeSet::ALL), action, &resource);
            if role.at_least(*floor) {
                let granted = outcome
                    .unwrap_or_else(|denial| panic!("{route:?} at {role:?} denied: {denial}"));
                assert_eq!(granted.role, Some(role));
                assert_eq!(granted.organization_id, Some(Uuid::from_u128(ORG)));
            } else {
                assert_eq!(
                    outcome,
                    Err(Denial::InsufficientRole { required: *floor }),
                    "{route:?} at {role:?}"
                );
            }
        }
    }
}

#[test]
fn a_non_member_is_denied_before_any_role_or_scope_check() {
    let stranger = Principal::AccountActor {
        user_id: Uuid::from_u128(USER),
        credential: ActorCredential::AccountToken(Uuid::from_u128(0x61)),
        memberships: Vec::new(),
        token_scopes: ScopeSet::ALL,
    };
    assert_eq!(
        decide(&stranger, action(RouteId::OrganizationGet), &organization()),
        Err(Denial::NotAMember)
    );
}

#[test]
fn non_path_targets_defer_membership_role_and_account_state_to_their_handlers() {
    for route_id in [
        RouteId::WorkspaceCreate,
        RouteId::ApiKeysList,
        RouteId::ApiKeyCreate,
    ] {
        let action = action(route_id);
        let requirement = requirement(action);
        assert_eq!(
            requirement.resource_class,
            ResourceClass::None,
            "{route_id:?}"
        );
        assert_eq!(requirement.min_role, None, "{route_id:?}");
        assert!(
            !route(route_id).pause_exempt,
            "{route_id:?} must gate account state after its target is decoded"
        );
        assert!(
            decide(
                &actor(OrgRole::Member, ScopeSet::ALL),
                action,
                &Resource::None
            )
            .is_ok(),
            "the edge checks the actor and scope; the handler checks the decoded target for {route_id:?}"
        );
    }
}

#[test]
fn a_membership_in_another_organization_does_not_carry_over() {
    let elsewhere = Principal::AccountActor {
        user_id: Uuid::from_u128(USER),
        credential: ActorCredential::AccountToken(Uuid::from_u128(0x61)),
        memberships: vec![OrgMembership {
            organization_id: Uuid::from_u128(OTHER_ORG),
            membership_id: Uuid::from_u128(0x72),
            role: OrgRole::Owner,
        }],
        token_scopes: ScopeSet::ALL,
    };
    assert_eq!(
        decide(
            &elsewhere,
            action(RouteId::OrganizationGet),
            &organization()
        ),
        Err(Denial::NotAMember)
    );
}

#[test]
fn a_token_missing_the_routes_scope_is_denied_even_as_an_owner() {
    let narrow = actor(
        OrgRole::Owner,
        ScopeSet::ALL.remove(ScopeId::WorkspacesDelete),
    );
    assert_eq!(
        decide(&narrow, action(RouteId::WorkspaceDelete), &workspace()),
        Err(Denial::InsufficientScope {
            required: ScopeId::WorkspacesDelete
        })
    );
}

#[test]
fn effective_scopes_are_the_intersection_of_token_and_role() {
    let granted = decide(
        &actor(OrgRole::Member, ScopeSet::ALL),
        action(RouteId::OrganizationGet),
        &organization(),
    )
    .expect("a member may read its organization");
    assert_eq!(granted.effective_scopes, ScopeSet::MEMBER);
    assert!(!granted.effective_scopes.contains(ScopeId::WorkspacesDelete));
}

#[test]
fn a_dashboard_session_and_an_account_token_are_the_same_principal() {
    for (route, class) in [
        (RouteId::OrganizationGet, ResourceClass::Organization),
        (RouteId::MembershipsList, ResourceClass::Organization),
        (RouteId::WorkspaceGet, ResourceClass::Workspace),
    ] {
        let resource = resource_for(class);
        let by_session = decide(&session_actor(OrgRole::Admin), action(route), &resource);
        let by_token = decide(
            &actor(OrgRole::Admin, ScopeSet::ALL),
            action(route),
            &resource,
        );
        assert_eq!(by_session, by_token, "{route:?}");
        assert!(by_session.is_ok(), "{route:?}");
    }
}

#[test]
fn a_workspace_key_is_refused_on_every_organization_route() {
    for route in [
        RouteId::OrganizationGet,
        RouteId::MembershipsList,
        RouteId::InvitationCreate,
        RouteId::WorkspaceCreate,
        RouteId::ApiKeysList,
        RouteId::ApiKeyCreate,
        RouteId::WorkspaceDelete,
    ] {
        let action = action(route);
        let resource = resource_for(requirement(action).resource_class);
        assert_eq!(
            decide(&key(ScopeSet::ALL), action, &resource),
            Err(Denial::WrongPrincipalKind {
                kind: PrincipalKindTag::WorkspaceKey
            }),
            "{route:?}"
        );
    }
}

#[test]
fn a_workspace_key_can_never_be_redirected_at_another_organization() {
    let granted = decide(
        &key(ScopeSet::ALL),
        action(RouteId::AccountGet),
        &Resource::None,
    )
    .expect("a key may read its own account");
    assert_eq!(granted.organization_id, Some(Uuid::from_u128(ORG)));
    assert_eq!(
        decide(
            &key(ScopeSet::ALL),
            action(RouteId::AccountGet),
            &Resource::Organization(Uuid::from_u128(OTHER_ORG))
        ),
        Err(Denial::WrongResourceClass),
        "a key cannot smuggle an organization into an actor-scoped route"
    );
}

#[test]
fn a_key_presenting_a_foreign_organization_or_workspace_is_refused() {
    // `billing_balance_get` is organization-free and key-admitting; the guard is
    // exercised directly against the two mismatching resources it could be
    // handed by a request field.
    let principal = key(ScopeSet::ALL);
    let action = action(RouteId::BillingBalanceGet);
    assert!(decide(&principal, action, &Resource::None).is_ok());

    // The same guard, evaluated by hand against the two mismatch shapes: the
    // resource class filter runs first, so a mismatching class is refused before
    // the binding check ever sees it.
    for foreign in [
        Resource::Organization(Uuid::from_u128(OTHER_ORG)),
        Resource::Workspace {
            workspace_id: Uuid::from_u128(OTHER_WORKSPACE),
            organization_id: Uuid::from_u128(OTHER_ORG),
        },
    ] {
        assert_eq!(
            decide(&principal, action, &foreign),
            Err(Denial::WrongResourceClass),
            "{foreign:?}"
        );
    }
}

#[test]
fn a_key_is_capped_at_the_mintable_ceiling_even_if_its_row_says_otherwise() {
    let granted = decide(
        &key(ScopeSet::ALL),
        action(RouteId::AccountGet),
        &Resource::None,
    )
    .expect("a key may read its own account");
    assert_eq!(
        granted.effective_scopes,
        ScopeSet::ALL.intersect(ScopeSet::WORKSPACE_KEY_MINTABLE)
    );
    assert!(!granted.effective_scopes.contains(ScopeId::ApiKeysWrite));
    assert!(
        !granted
            .effective_scopes
            .contains(ScopeId::OrganizationsRead)
    );
}

#[test]
fn an_anonymous_principal_reaches_exactly_the_three_ceremony_entry_routes() {
    for action in Action::all() {
        let outcome = decide(&Principal::Anonymous, action, &Resource::None);
        if requirement(action)
            .principal_kinds
            .admits(PrincipalKindTag::Anonymous)
        {
            assert!(outcome.is_ok(), "{:?}", action.route());
        } else {
            assert_eq!(
                outcome,
                Err(Denial::NotAuthenticated),
                "{:?}",
                action.route()
            );
        }
    }
    let anonymous: Vec<String> = Action::all()
        .into_iter()
        .filter(|action| {
            requirement(*action)
                .principal_kinds
                .admits(PrincipalKindTag::Anonymous)
        })
        .map(|action| format!("{:?}", action.route()))
        .collect();
    // Three, not two. `DashboardSessionCreate` is the ceremony's other entry
    // point: nobody holds an AEX credential before it answers, so it cannot
    // require one. It proves its caller with the first-party exchange secret in
    // its body — the same shape as `DeviceTokenCreate`, whose device code is
    // also a credential the edge never sees.
    assert_eq!(
        anonymous,
        vec![
            "DashboardSessionCreate".to_owned(),
            "DeviceAuthorizationCreate".to_owned(),
            "DeviceTokenCreate".to_owned()
        ]
    );
}

#[test]
fn a_paused_account_reaches_only_the_pause_exempt_routes() {
    for action in Action::all() {
        let requirement = requirement(action);
        let resource = resource_for(requirement.resource_class);
        let principal = if requirement
            .principal_kinds
            .admits(PrincipalKindTag::Anonymous)
        {
            Principal::Anonymous
        } else {
            actor(OrgRole::Owner, ScopeSet::ALL)
        };
        let outcome = admit(
            &principal,
            action,
            &resource,
            AccountState::PausedTopUpRequired,
        );
        if requirement.pause_exempt {
            assert!(outcome.is_ok(), "{:?} is pause-exempt", action.route());
        } else {
            assert_eq!(
                outcome,
                Err(Admission::Paused),
                "{:?} is not pause-exempt",
                action.route()
            );
        }
    }
}

#[test]
fn an_unreadable_account_state_is_a_refusal_and_never_an_assumption() {
    let action = action(RouteId::InvitationCreate);
    assert_eq!(
        admit(
            &actor(OrgRole::Admin, ScopeSet::ALL),
            action,
            &organization(),
            AccountState::Unavailable
        ),
        Err(Admission::StateUnavailable)
    );
    assert!(
        admit(
            &actor(OrgRole::Admin, ScopeSet::ALL),
            action,
            &organization(),
            AccountState::Active
        )
        .is_ok()
    );
}

#[test]
fn authorization_is_decided_before_the_account_state_gate() {
    assert_eq!(
        admit(
            &actor(OrgRole::Member, ScopeSet::ALL),
            action(RouteId::InvitationCreate),
            &organization(),
            AccountState::PausedTopUpRequired
        ),
        Err(Admission::Denied(Denial::InsufficientRole {
            required: OrgRole::Admin
        })),
        "a 403 is decided before a 402"
    );
}

#[test]
fn a_mismatched_resource_class_is_refused() {
    assert_eq!(
        decide(
            &actor(OrgRole::Owner, ScopeSet::ALL),
            action(RouteId::WorkspaceGet),
            &organization()
        ),
        Err(Denial::WrongResourceClass)
    );
    assert_eq!(
        decide(
            &actor(OrgRole::Owner, ScopeSet::ALL),
            action(RouteId::OrganizationGet),
            &Resource::None
        ),
        Err(Denial::WrongResourceClass)
    );
}

#[test]
fn every_central_route_is_reachable_by_at_least_one_principal() {
    for action in Action::all() {
        let resource = resource_for(requirement(action).resource_class);
        let candidates = [
            Principal::Anonymous,
            actor(OrgRole::Owner, ScopeSet::ALL),
            key(ScopeSet::ALL),
        ];
        assert!(
            candidates
                .iter()
                .any(|principal| decide(principal, action, &resource).is_ok()),
            "{:?} is unreachable",
            action.route()
        );
    }
}

#[test]
fn every_route_requiring_a_scope_names_one_the_admitted_principal_can_hold() {
    for action in Action::all() {
        let requirement = requirement(action);
        let Some(scope) = requirement.scope else {
            continue;
        };
        if requirement
            .principal_kinds
            .admits(PrincipalKindTag::WorkspaceKey)
        {
            assert!(
                ScopeSet::WORKSPACE_KEY_MINTABLE.contains(scope),
                "{:?} admits a workspace key but requires `{scope}`, which a key may never carry",
                action.route()
            );
        }
        assert!(
            ScopeSet::OWNER.contains(scope),
            "{:?} requires `{scope}`, which no role carries",
            action.route()
        );
    }
}
