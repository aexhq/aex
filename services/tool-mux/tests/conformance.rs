//! Service-side private scope conformance.

use aex_wire::ids::{OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};

#[test]
fn private_scope_preserves_the_exact_tenant_and_session() {
    let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10]));
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(2, [2; 10]));
    let session = SessionId::from_uuid7(Uuid7::compose(3, [3; 10]));
    let scope = tool_mux::auth::scope(session, Some(organization), Some(workspace));
    assert_eq!(scope.session, session);
    assert_eq!(scope.organization, Some(organization));
    assert_eq!(scope.workspace, Some(workspace));
}
