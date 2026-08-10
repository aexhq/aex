//! The one authorization decision.
//!
//! Every central route runs through [`decide`] and then [`admit`]. This edge
//! decision owns principal-kind and scope checks. A collection route whose
//! organization exists only in a typed query or body stays actor-scoped here; its
//! handler must load that organization and enforce membership, role and account
//! state from the control authority before it reads or changes tenant data.
//!
//! Two invariants close standing defects by construction.
//!
//! 1. A workspace key's organization and workspace come from its own row.
//!    A request naming a different organization or workspace is
//!    [`Denial::WrongOrganization`] or [`Denial::WrongWorkspace`] — never a
//!    silent narrowing to whatever the row says.
//! 2. Every route that is not pause-exempt either resolves an organization at
//!    the edge or belongs to the explicit non-path, handler-owned set. A route
//!    in that set resolves and gates its decoded target in the handler when it
//!    acts on one organization. The partition is asserted over the whole
//!    generated central route table.

use aex_wire::routes::{Plane, ROUTES, RouteDescriptor, RouteId};

use crate::scope::{Scope, ScopeSet};
use uuid::Uuid;

/// A person's role inside one organization.
///
/// One enum for every surface. The system this replaces had bootstrap writing
/// `owner`, the dashboard writing `admin`, and a parser that threw on `owner` —
/// so each surface froze the other out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrgRole {
    /// Read-mostly.
    Member,
    /// Everything except the irreversible.
    Admin,
    /// Everything.
    Owner,
}

impl OrgRole {
    /// Every role, weakest first.
    pub const ALL: [Self; 3] = [Self::Member, Self::Admin, Self::Owner];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == text)
    }

    /// The central scopes this role carries.
    #[must_use]
    pub const fn scopes(self) -> ScopeSet {
        match self {
            Self::Member => ScopeSet::MEMBER,
            Self::Admin => ScopeSet::ADMIN,
            Self::Owner => ScopeSet::OWNER,
        }
    }

    /// Whether this role is at least `minimum`.
    #[must_use]
    pub const fn at_least(self, minimum: Self) -> bool {
        self as u8 >= minimum as u8
    }
}

/// Which credential a person presented.
///
/// Both arms are the same principal with the same scopes and the same role. The
/// distinction exists for audit and revocation, not for admission: one credential
/// path, not two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorCredential {
    /// A browser session minted by the dashboard sign-in ceremony.
    DashboardSession(Uuid),
    /// An account token minted by the device flow.
    AccountToken(Uuid),
}

impl ActorCredential {
    /// The credential's own id.
    #[must_use]
    pub const fn id(self) -> Uuid {
        match self {
            Self::DashboardSession(id) | Self::AccountToken(id) => id,
        }
    }
}

/// One membership the actor holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrgMembership {
    /// Which organization.
    pub organization_id: Uuid,
    /// The membership row.
    pub membership_id: Uuid,
    /// The role held.
    pub role: OrgRole,
}

/// The authenticated principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// No credential at all. Only the three anonymous ceremony routes admit this.
    Anonymous,
    /// A person, through either credential.
    AccountActor {
        /// Who.
        user_id: Uuid,
        /// Which credential.
        credential: ActorCredential,
        /// Every active membership, as read in the same statement.
        memberships: Vec<OrgMembership>,
        /// The scopes the credential itself carries.
        token_scopes: ScopeSet,
    },
    /// A workspace API key, pinned to one workspace and one region.
    WorkspaceKey {
        /// The key row.
        key_id: Uuid,
        /// The workspace it belongs to, from its own row.
        workspace_id: Uuid,
        /// The organization it belongs to, from its own row.
        organization_id: Uuid,
        /// The scopes the key carries.
        scopes: ScopeSet,
    },
}

/// The principal kind, without the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrincipalKindTag {
    /// [`Principal::Anonymous`].
    Anonymous,
    /// [`Principal::AccountActor`].
    AccountActor,
    /// [`Principal::WorkspaceKey`].
    WorkspaceKey,
}

impl PrincipalKindTag {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::Anonymous, Self::AccountActor, Self::WorkspaceKey];

    /// The database and telemetry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::AccountActor => "account_actor",
            Self::WorkspaceKey => "workspace_key",
        }
    }

    /// The bit this kind occupies in a [`PrincipalKinds`] set.
    #[must_use]
    const fn bit(self) -> u8 {
        1_u8 << (self as u8)
    }
}

impl Principal {
    /// The principal's kind.
    #[must_use]
    pub const fn kind(&self) -> PrincipalKindTag {
        match self {
            Self::Anonymous => PrincipalKindTag::Anonymous,
            Self::AccountActor { .. } => PrincipalKindTag::AccountActor,
            Self::WorkspaceKey { .. } => PrincipalKindTag::WorkspaceKey,
        }
    }

    /// The principal's own id, for the replay identity and the audit row.
    #[must_use]
    pub const fn id(&self) -> Option<Uuid> {
        match self {
            Self::Anonymous => None,
            Self::AccountActor { credential, .. } => Some(credential.id()),
            Self::WorkspaceKey { key_id, .. } => Some(*key_id),
        }
    }
}

/// The principal kinds a route admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrincipalKinds(u8);

impl PrincipalKinds {
    /// Only a person.
    pub const ACTOR: Self = Self(PrincipalKindTag::AccountActor.bit());
    /// A person or a workspace key.
    pub const ACTOR_OR_KEY: Self =
        Self(PrincipalKindTag::AccountActor.bit() | PrincipalKindTag::WorkspaceKey.bit());
    /// No credential at all.
    pub const ANONYMOUS: Self = Self(PrincipalKindTag::Anonymous.bit());

    /// Whether the set admits `kind`.
    #[must_use]
    pub const fn admits(self, kind: PrincipalKindTag) -> bool {
        self.0 & kind.bit() != 0
    }
}

/// What kind of resource a route acts on.
///
/// `None` means the route resolves no organization, which is what makes it
/// structurally pause-exempt: there is no account whose state could gate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceClass {
    /// The route is actor-scoped and resolves no organization.
    None,
    /// One organization.
    Organization,
    /// One workspace, inside one organization.
    Workspace,
    /// One API key, inside one workspace.
    ApiKey,
    /// One durable operation, inside one organization.
    Operation,
}

/// The resolved resource a request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    /// No resource; the route is actor-scoped.
    None,
    /// One organization.
    Organization(Uuid),
    /// One workspace.
    Workspace {
        /// The workspace.
        workspace_id: Uuid,
        /// Its organization, read from the workspace row.
        organization_id: Uuid,
    },
    /// One API key.
    ApiKey {
        /// The key.
        key_id: Uuid,
        /// Its workspace, read from the key row.
        workspace_id: Uuid,
        /// Its organization, read from the key row.
        organization_id: Uuid,
    },
    /// One durable operation.
    Operation {
        /// The operation.
        operation_id: Uuid,
        /// Its organization, read from the operation row.
        organization_id: Uuid,
    },
}

impl Resource {
    /// Which class this resource belongs to.
    #[must_use]
    pub const fn class(&self) -> ResourceClass {
        match self {
            Self::None => ResourceClass::None,
            Self::Organization(_) => ResourceClass::Organization,
            Self::Workspace { .. } => ResourceClass::Workspace,
            Self::ApiKey { .. } => ResourceClass::ApiKey,
            Self::Operation { .. } => ResourceClass::Operation,
        }
    }

    /// The organization this resource belongs to, when there is one.
    #[must_use]
    pub const fn organization(&self) -> Option<Uuid> {
        match self {
            Self::None => None,
            Self::Organization(id)
            | Self::Workspace {
                organization_id: id,
                ..
            }
            | Self::ApiKey {
                organization_id: id,
                ..
            }
            | Self::Operation {
                organization_id: id,
                ..
            } => Some(*id),
        }
    }

    /// The workspace this resource belongs to, when there is one.
    #[must_use]
    pub const fn workspace(&self) -> Option<Uuid> {
        match self {
            Self::None | Self::Organization(_) | Self::Operation { .. } => None,
            Self::Workspace { workspace_id, .. } | Self::ApiKey { workspace_id, .. } => {
                Some(*workspace_id)
            }
        }
    }
}

/// Why an action could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("`{route:?}` is not a central-plane route")]
pub struct NotCentral {
    /// The route that was offered.
    pub route: RouteId,
}

/// One central-plane operation, as an authorization subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Action(RouteId);

impl Action {
    /// Wraps a central route.
    ///
    /// # Errors
    ///
    /// Returns [`NotCentral`] for a regional route, so a regional operation can
    /// never be evaluated against the central role model by accident.
    pub fn central(route: RouteId) -> Result<Self, NotCentral> {
        if descriptor_of(route).plane == Plane::Central {
            Ok(Self(route))
        } else {
            Err(NotCentral { route })
        }
    }

    /// The underlying route.
    #[must_use]
    pub const fn route(self) -> RouteId {
        self.0
    }

    /// The generated descriptor.
    #[must_use]
    pub fn descriptor(self) -> &'static RouteDescriptor {
        descriptor_of(self.0)
    }

    /// Every central action, in route-table order.
    #[must_use]
    pub fn all() -> Vec<Self> {
        ROUTES
            .iter()
            .filter(|route| route.plane == Plane::Central)
            .map(|route| Self(route.id))
            .collect()
    }
}

/// The generated descriptor for `route`.
///
/// # Panics
///
/// Never: `ROUTES` holds one descriptor per `RouteId` by construction, and
/// `aex-wire`'s own suite asserts it.
fn descriptor_of(route: RouteId) -> &'static RouteDescriptor {
    aex_wire::routes::route(route)
}

/// The role floor and resource class of one central route.
struct Rule {
    /// Which route.
    route: RouteId,
    /// What it acts on.
    class: ResourceClass,
    /// The weakest role that may run it, when an organization is resolved.
    min_role: Option<OrgRole>,
}

/// The central authorization matrix.
///
/// `scope`, `pause_exempt` and the admitted principal kinds are **not** here:
/// they are read from the generated route table so the two cannot disagree.
/// What lives here is the one fact the wire contract does not carry — the role
/// floor — plus the resource class the floor is evaluated against.
const RULES: &[Rule] = &[
    // --- identity and account ------------------------------------------------
    Rule {
        route: RouteId::DeviceAuthorizationCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::DeviceTokenCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    // The three credential-ceremony routes are account-shaped, not
    // organization-shaped: a person decides their own device authorization and
    // closes their own browser session before any organization has been
    // selected, so there is no resource to classify and no membership role a
    // floor could require.
    Rule {
        route: RouteId::DeviceDecisionCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::DashboardSessionCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::DashboardSessionDelete,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::DashboardBootstrapGet,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::AccountGet,
        class: ResourceClass::None,
        min_role: None,
    },
    // --- organizations and memberships --------------------------------------
    Rule {
        route: RouteId::OrganizationsList,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::OrganizationCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::OrganizationGet,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::MembershipsList,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::InvitationCreate,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Admin),
    },
    // --- workspaces ----------------------------------------------------------
    Rule {
        route: RouteId::WorkspacesList,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::WorkspaceCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::WorkspaceGet,
        class: ResourceClass::Workspace,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::WorkspaceDelete,
        class: ResourceClass::Workspace,
        min_role: Some(OrgRole::Owner),
    },
    // --- API keys ------------------------------------------------------------
    Rule {
        route: RouteId::ApiKeysList,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::ApiKeyCreate,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::ApiKeyRevoke,
        class: ResourceClass::ApiKey,
        min_role: Some(OrgRole::Admin),
    },
    // --- durable operations --------------------------------------------------
    Rule {
        route: RouteId::CentralOperationsList,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::CentralOperationGet,
        class: ResourceClass::Operation,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::CentralOperationCancel,
        class: ResourceClass::Operation,
        min_role: Some(OrgRole::Member),
    },
    // --- billing (finance stream's handlers, this plane's admission) ---------
    Rule {
        route: RouteId::BillingBalanceGet,
        class: ResourceClass::None,
        min_role: None,
    },
    Rule {
        route: RouteId::BillingStatementsList,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::BillingStatementGet,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::BillingStatementDownloadCreate,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::BillingAutoTopupPolicyGet,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Member),
    },
    Rule {
        route: RouteId::BillingAutoTopupPolicyPut,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Admin),
    },
    Rule {
        route: RouteId::BillingPortalSessionCreate,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Admin),
    },
    Rule {
        route: RouteId::BillingTopUpCheckoutCreate,
        class: ResourceClass::Organization,
        min_role: Some(OrgRole::Admin),
    },
];

/// Everything admission needs to know about one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// The scope the credential must carry, from the generated route table.
    pub scope: Option<Scope>,
    /// The weakest role that may run it, when an organization is resolved.
    pub min_role: Option<OrgRole>,
    /// Which principal kinds the route admits.
    pub principal_kinds: PrincipalKinds,
    /// What the route acts on.
    pub resource_class: ResourceClass,
    /// Whether the edge skips its account-state read.
    ///
    /// This is not necessarily the generated route's pause policy. A route that
    /// resolves no organization at the edge skips this read by construction;
    /// when its descriptor is not pause-exempt, the handler must gate account
    /// state after decoding its non-path target. Everything else takes the
    /// generated route table's answer verbatim.
    pub pause_exempt: bool,
}

/// The requirement for one central action.
///
/// # Panics
///
/// Never: [`RULES`] covers exactly the central route table, which
/// `the_rule_table_covers_exactly_the_central_route_table` asserts.
#[must_use]
pub fn requirement(action: Action) -> Requirement {
    let descriptor = action.descriptor();
    let rule = RULES
        .iter()
        .find(|rule| rule.route == action.route())
        .expect("every central route has a rule");
    Requirement {
        scope: descriptor.required_scope,
        min_role: rule.min_role,
        principal_kinds: admitted_kinds(descriptor),
        resource_class: rule.class,
        pause_exempt: descriptor.pause_exempt || rule.class == ResourceClass::None,
    }
}

/// The principal kinds a descriptor admits.
///
/// The wire contract's `account` and `user_session` are the same principal here
/// — same person, same scopes, same role, different credential — so both map
/// onto [`PrincipalKindTag::AccountActor`]. An `anonymous` alternative replaces
/// the base kind rather than adding to it, because a route that admits no
/// credential also requires no scope.
fn admitted_kinds(descriptor: &RouteDescriptor) -> PrincipalKinds {
    match descriptor.alt_principal {
        Some(aex_wire::routes::PrincipalKind::Anonymous) => PrincipalKinds::ANONYMOUS,
        Some(aex_wire::routes::PrincipalKind::WorkspaceKey) => PrincipalKinds::ACTOR_OR_KEY,
        Some(
            aex_wire::routes::PrincipalKind::Account | aex_wire::routes::PrincipalKind::UserSession,
        )
        | None => PrincipalKinds::ACTOR,
    }
}

/// Why a principal may not act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Denial {
    /// No credential was presented on a route that requires one.
    #[error("no credential was presented")]
    NotAuthenticated,
    /// The actor holds no active membership in the resolved organization.
    #[error("the actor is not a member of that organization")]
    NotAMember,
    /// The actor's role is below the route's floor.
    #[error("the route requires the `{required}` role or stronger", required = .required.as_str())]
    InsufficientRole {
        /// The weakest role that would have been admitted.
        required: OrgRole,
    },
    /// The effective scopes do not carry the route's scope.
    #[error("the credential lacks `{required}`")]
    InsufficientScope {
        /// The scope the route requires.
        required: Scope,
    },
    /// The route does not admit this principal kind.
    #[error("the route does not admit a `{kind}` principal", kind = .kind.as_str())]
    WrongPrincipalKind {
        /// What was presented.
        kind: PrincipalKindTag,
    },
    /// A workspace key named an organization other than its own.
    #[error("the credential belongs to a different organization")]
    WrongOrganization,
    /// A workspace key named a workspace other than its own.
    #[error("the credential belongs to a different workspace")]
    WrongWorkspace,
    /// The resolved resource is not the class the route acts on.
    #[error("the route acts on a different kind of resource")]
    WrongResourceClass,
}

/// What a principal was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Granted {
    /// The organization the request acts in, when the route resolves one.
    pub organization_id: Option<Uuid>,
    /// The scopes that actually apply, already intersected with role and with
    /// the workspace-key ceiling. The edge never re-derives these.
    pub effective_scopes: ScopeSet,
    /// The role the actor holds in the resolved organization.
    pub role: Option<OrgRole>,
}

/// Decides whether `principal` may run `action` on `resource`.
///
/// # Errors
///
/// Returns [`Denial`] naming the first rule that refused, in the fixed order:
/// principal kind, resource class, credential binding, membership, role, scope.
/// Ordering the checks means a caller learns the least possible about state it
/// is not entitled to see.
pub fn decide(
    principal: &Principal,
    action: Action,
    resource: &Resource,
) -> Result<Granted, Denial> {
    let requirement = requirement(action);

    if !requirement.principal_kinds.admits(principal.kind()) {
        return Err(match principal {
            Principal::Anonymous => Denial::NotAuthenticated,
            _ => Denial::WrongPrincipalKind {
                kind: principal.kind(),
            },
        });
    }
    if resource.class() != requirement.resource_class {
        return Err(Denial::WrongResourceClass);
    }

    match principal {
        Principal::Anonymous => Ok(Granted {
            organization_id: None,
            effective_scopes: ScopeSet::EMPTY,
            role: None,
        }),
        Principal::WorkspaceKey {
            workspace_id,
            organization_id,
            scopes,
            ..
        } => {
            if let Some(named) = resource.organization()
                && named != *organization_id
            {
                return Err(Denial::WrongOrganization);
            }
            if let Some(named) = resource.workspace()
                && named != *workspace_id
            {
                return Err(Denial::WrongWorkspace);
            }
            let effective = scopes.intersect(ScopeSet::WORKSPACE_KEY_MINTABLE);
            check_scope(requirement.scope, effective)?;
            Ok(Granted {
                organization_id: Some(*organization_id),
                effective_scopes: effective,
                role: None,
            })
        }
        Principal::AccountActor {
            memberships,
            token_scopes,
            ..
        } => {
            let Some(organization_id) = resource.organization() else {
                // An actor-scoped route: no organization, therefore no role to
                // narrow with. The handler filters its own rows by membership.
                let effective = token_scopes.intersect(ScopeSet::CENTRAL);
                check_scope(requirement.scope, effective)?;
                return Ok(Granted {
                    organization_id: None,
                    effective_scopes: effective,
                    role: None,
                });
            };
            let membership = memberships
                .iter()
                .find(|membership| membership.organization_id == organization_id)
                .ok_or(Denial::NotAMember)?;
            if let Some(minimum) = requirement.min_role
                && !membership.role.at_least(minimum)
            {
                return Err(Denial::InsufficientRole { required: minimum });
            }
            let effective = token_scopes.intersect(membership.role.scopes());
            check_scope(requirement.scope, effective)?;
            Ok(Granted {
                organization_id: Some(organization_id),
                effective_scopes: effective,
                role: Some(membership.role),
            })
        }
    }
}

/// Checks the route's scope against the effective set.
fn check_scope(required: Option<Scope>, effective: ScopeSet) -> Result<(), Denial> {
    match required {
        Some(scope) if !effective.contains(scope) => {
            Err(Denial::InsufficientScope { required: scope })
        }
        _ => Ok(()),
    }
}

/// The billing state of an organization's account.
///
/// `Unavailable` is a first-class arm, not an absent value: the authorization
/// read `COALESCE`s a missing `finance.account_state_v1` row to it, and it maps
/// to `503 account_state_unavailable`. An absent row is never "active".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountState {
    /// Admission proceeds.
    Active,
    /// The account owes a top-up; only pause-exempt routes run.
    PausedTopUpRequired,
    /// The state could not be established.
    Unavailable,
}

impl AccountState {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::PausedTopUpRequired => "paused_top_up_required",
            Self::Unavailable => "unavailable",
        }
    }

    /// Resolves a database spelling, including the `COALESCE` placeholder.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "paused_top_up_required" | "paused" => Some(Self::PausedTopUpRequired),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// Why admission stopped after authorization succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Admission {
    /// The principal may not act at all.
    #[error(transparent)]
    Denied(#[from] Denial),
    /// The account is paused and the route is not pause-exempt.
    #[error("the account is paused")]
    Paused,
    /// The account state could not be established.
    #[error("the account state is unavailable")]
    StateUnavailable,
}

/// Decides, then applies the account-state gate.
///
/// The order is fixed and matches the wire contract's error precedence: a `403`
/// is decided before a `402`, and an unreadable account state is a `503` rather
/// than an assumption in either direction.
///
/// # Errors
///
/// Returns [`Admission::Denied`] for an authorization failure,
/// [`Admission::Paused`] for a paused account on a non-exempt route, and
/// [`Admission::StateUnavailable`] when the state could not be read.
pub fn admit(
    principal: &Principal,
    action: Action,
    resource: &Resource,
    state: AccountState,
) -> Result<Granted, Admission> {
    let granted = decide(principal, action, resource)?;
    if requirement(action).pause_exempt {
        return Ok(granted);
    }
    match state {
        AccountState::Active => Ok(granted),
        AccountState::PausedTopUpRequired => Err(Admission::Paused),
        AccountState::Unavailable => Err(Admission::StateUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, OrgRole, PrincipalKindTag, RULES, ResourceClass, requirement};
    use aex_wire::routes::{Plane, ROUTES, RouteId};
    use std::collections::BTreeSet;

    const fn handler_owns_non_path_target(route: RouteId) -> bool {
        matches!(
            route,
            RouteId::OrganizationsList
                | RouteId::OrganizationCreate
                | RouteId::WorkspacesList
                | RouteId::WorkspaceCreate
                | RouteId::ApiKeysList
                | RouteId::ApiKeyCreate
                | RouteId::CentralOperationsList
        )
    }

    #[test]
    fn the_rule_table_covers_exactly_the_central_route_table() {
        let declared: BTreeSet<_> = RULES
            .iter()
            .map(|rule| format!("{:?}", rule.route))
            .collect();
        let central: BTreeSet<_> = ROUTES
            .iter()
            .filter(|route| route.plane == Plane::Central)
            .map(|route| format!("{:?}", route.id))
            .collect();
        assert_eq!(
            declared, central,
            "every central route needs exactly one rule"
        );
        assert_eq!(RULES.len(), declared.len(), "no route is ruled twice");
    }

    #[test]
    fn every_non_exempt_route_resolves_an_organization_or_names_a_handler_owned_target() {
        for action in Action::all() {
            let requirement = requirement(action);
            let descriptor = aex_wire::routes::route(action.route());
            assert!(
                descriptor.pause_exempt
                    || requirement.resource_class != ResourceClass::None
                    || handler_owns_non_path_target(action.route()),
                "{:?} is not pause-exempt, resolves no organization at the edge, and names no handler-owned non-path target",
                action.route()
            );
        }
    }

    #[test]
    fn a_role_floor_exists_exactly_where_an_organization_is_resolved() {
        for action in Action::all() {
            let requirement = requirement(action);
            assert_eq!(
                requirement.min_role.is_some(),
                requirement.resource_class != ResourceClass::None,
                "{:?} pairs a role floor with a resolved organization",
                action.route()
            );
        }
    }

    #[test]
    fn non_path_targets_defer_their_organization_gate_to_the_handler() {
        for route_id in [
            RouteId::WorkspaceCreate,
            RouteId::ApiKeysList,
            RouteId::ApiKeyCreate,
        ] {
            let requirement = requirement(Action::central(route_id).expect("central"));
            assert_eq!(
                requirement.resource_class,
                ResourceClass::None,
                "{route_id:?}"
            );
            assert_eq!(requirement.min_role, None, "{route_id:?}");
            assert!(
                !aex_wire::routes::route(route_id).pause_exempt,
                "{route_id:?} must gate account state after its target is decoded"
            );
        }
    }

    #[test]
    fn an_anonymous_route_requires_no_scope() {
        for action in Action::all() {
            let requirement = requirement(action);
            if requirement
                .principal_kinds
                .admits(PrincipalKindTag::Anonymous)
            {
                assert_eq!(
                    requirement.scope,
                    None,
                    "{:?} admits no credential yet requires a scope",
                    action.route()
                );
            }
        }
    }

    #[test]
    fn a_regional_route_is_not_a_central_action() {
        let regional = ROUTES
            .iter()
            .find(|route| route.plane == Plane::Regional)
            .expect("the regional plane is not empty");
        assert!(Action::central(regional.id).is_err());
    }

    #[test]
    fn roles_are_ordered_weakest_first() {
        assert!(OrgRole::Owner.at_least(OrgRole::Member));
        assert!(OrgRole::Owner.at_least(OrgRole::Admin));
        assert!(OrgRole::Admin.at_least(OrgRole::Member));
        assert!(!OrgRole::Member.at_least(OrgRole::Admin));
        assert!(!OrgRole::Admin.at_least(OrgRole::Owner));
        for role in OrgRole::ALL {
            assert_eq!(OrgRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(OrgRole::parse("root"), None);
    }
}
