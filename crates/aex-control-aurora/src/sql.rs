//! Every control statement, as a `const &str`.
//!
//! There is no format-string SQL and no identifier interpolation in this crate.
//! A statement is a constant a reviewer can read whole, and `sql_discipline.rs`
//! asserts that property over the source rather than trusting it.
//!
//! Every `timestamptz` column is projected as `(EXTRACT(EPOCH FROM col)*1000)::bigint`
//! and every timestamp parameter is bound as epoch milliseconds through
//! `TIMESTAMPTZ 'epoch' + :p * INTERVAL '1 millisecond'`. The system this
//! replaces returned naive `"2026-07-04 12:34:56.789"` strings and re-bound them
//! as cursor values, which is correct only while the session time zone happens
//! to be UTC.

/// Resolve a workspace API key for an assertion issue.
///
/// **One statement, no transaction.** Everything the issuer needs — the key, the
/// verifier, its pepper version, the workspace, the organization, the finance
/// account state and all three epochs — arrives in one round trip, which is the
/// pinned per-refresh I/O budget.
///
/// `COALESCE(a.status,'unavailable')` is deliberate: an absent
/// `finance.account_state_v1` row is **not** "active". It maps to
/// `503 account_state_unavailable`, which is the honest answer to "we could not
/// establish whether this account may spend".
pub const RESOLVE_WORKSPACE_KEY: &str = "\
SELECT k.id, k.workspace_id, k.organization_id, k.scopes, k.verifier, k.pepper_version, \
       (k.revoked_at IS NOT NULL) AS key_revoked, \
       w.region, w.status AS workspace_status, o.status AS organization_status, \
       COALESCE(a.status,'unavailable') AS account_status, \
       COALESCE(ek.epoch,0) AS epoch_key, \
       COALESCE(ew.epoch,0) AS epoch_workspace, \
       COALESCE(ea.epoch,0) AS epoch_account \
  FROM control.api_key k \
  JOIN control.workspace w ON w.id = k.workspace_id \
  JOIN control.organization o ON o.id = k.organization_id \
  LEFT JOIN finance.account_state_v1 a ON a.organization_id = k.organization_id \
  LEFT JOIN control.authorization_epoch ek ON ek.subject_kind='key'       AND ek.subject_id = k.id \
  LEFT JOIN control.authorization_epoch ew ON ew.subject_kind='workspace' AND ew.subject_id = w.id \
  LEFT JOIN control.authorization_epoch ea ON ea.subject_kind='account'   AND ea.subject_id = o.id \
 WHERE k.id = :key_id";

/// Resolve an account token scoped to one workspace, for an assertion issue.
///
/// The same shape as [`RESOLVE_WORKSPACE_KEY`], joined through
/// `identity.account_token → identity.user → control.membership`, and returning
/// the two extra epochs a person's assertion carries.
pub const RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE: &str = "\
SELECT t.id, t.user_id, m.id AS membership_id, m.role, t.scopes, t.verifier, t.pepper_version, \
       (t.revoked_at IS NOT NULL) AS credential_revoked, \
       (t.expires_at <= (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')) AS credential_expired, \
       (u.status = 'active') AS user_active, \
       w.id AS workspace_id, w.organization_id, w.region, \
       COALESCE(a.status,'unavailable') AS account_status, \
       COALESCE(eu.epoch,0) AS epoch_user, \
       COALESCE(em.epoch,0) AS epoch_membership, \
       COALESCE(ew.epoch,0) AS epoch_workspace, \
       COALESCE(ea.epoch,0) AS epoch_account \
  FROM identity.account_token t \
  JOIN identity.user u ON u.id = t.user_id \
  JOIN control.workspace w ON w.id = :workspace_id \
  JOIN control.membership m ON m.organization_id = w.organization_id \
                           AND m.user_id = t.user_id AND m.status = 'active' \
  LEFT JOIN finance.account_state_v1 a ON a.organization_id = w.organization_id \
  LEFT JOIN control.authorization_epoch eu ON eu.subject_kind='user'       AND eu.subject_id = u.id \
  LEFT JOIN control.authorization_epoch em ON em.subject_kind='membership' AND em.subject_id = m.id \
  LEFT JOIN control.authorization_epoch ew ON ew.subject_kind='workspace'  AND ew.subject_id = w.id \
  LEFT JOIN control.authorization_epoch ea ON ea.subject_kind='account'    AND ea.subject_id = w.organization_id \
 WHERE t.id = :credential_id";

/// Resolve a browser session scoped to one workspace, for an assertion issue.
///
/// Identical to [`RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE`] apart from the
/// credential table. That sameness is the point: a browser session and an
/// account token are the same principal reaching the same regional surface, so
/// they take one credential path rather than two. Without this statement every
/// regional dashboard panel is unimplementable.
pub const RESOLVE_SESSION_FOR_WORKSPACE: &str = "\
SELECT s.id, s.user_id, m.id AS membership_id, m.role, \
       ARRAY(SELECT unnest(ARRAY[]::text[])) AS scopes, s.verifier, s.pepper_version, \
       (s.revoked_at IS NOT NULL) AS credential_revoked, \
       (s.expires_at <= (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')) AS credential_expired, \
       (u.status = 'active') AS user_active, \
       w.id AS workspace_id, w.organization_id, w.region, \
       COALESCE(a.status,'unavailable') AS account_status, \
       COALESCE(eu.epoch,0) AS epoch_user, \
       COALESCE(em.epoch,0) AS epoch_membership, \
       COALESCE(ew.epoch,0) AS epoch_workspace, \
       COALESCE(ea.epoch,0) AS epoch_account \
  FROM identity.dashboard_session s \
  JOIN identity.user u ON u.id = s.user_id \
  JOIN control.workspace w ON w.id = :workspace_id \
  JOIN control.membership m ON m.organization_id = w.organization_id \
                           AND m.user_id = s.user_id AND m.status = 'active' \
  LEFT JOIN finance.account_state_v1 a ON a.organization_id = w.organization_id \
  LEFT JOIN control.authorization_epoch eu ON eu.subject_kind='user'       AND eu.subject_id = u.id \
  LEFT JOIN control.authorization_epoch em ON em.subject_kind='membership' AND em.subject_id = m.id \
  LEFT JOIN control.authorization_epoch ew ON ew.subject_kind='workspace'  AND ew.subject_id = w.id \
  LEFT JOIN control.authorization_epoch ea ON ea.subject_kind='account'    AND ea.subject_id = w.organization_id \
 WHERE s.id = :credential_id";

/// Resolves an account token and every active organization membership in one read.
pub const RESOLVE_ACCOUNT_TOKEN_CENTRAL: &str = "\
SELECT t.id, t.user_id, t.scopes, t.verifier, t.pepper_version, \
       (t.revoked_at IS NOT NULL) AS credential_revoked, \
       (t.expires_at <= (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')) AS credential_expired, \
       (u.status = 'active') AS user_active, \
       COALESCE((SELECT jsonb_agg(jsonb_build_array(m.organization_id::text, m.id::text, m.role) \
                                  ORDER BY m.id) \
                   FROM (SELECT organization_id, id, role FROM control.membership \
                          WHERE user_id = u.id AND status = 'active' ORDER BY id LIMIT 1001) m), \
                '[]'::jsonb) AS memberships \
  FROM identity.account_token t JOIN identity.user u ON u.id = t.user_id \
 WHERE t.id = :credential_id";

/// Resolves a dashboard session and every active organization membership in one read.
///
/// A dashboard session only enters the generated bootstrap route. Its one
/// route scope is assigned from the generated registry by the Rust adapter,
/// rather than repeated as SQL data here.
pub const RESOLVE_SESSION_CENTRAL: &str = "\
SELECT s.id, s.user_id, ARRAY(SELECT unnest(ARRAY[]::text[])) AS scopes, \
       s.verifier, s.pepper_version, (s.revoked_at IS NOT NULL) AS credential_revoked, \
       (s.expires_at <= (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')) AS credential_expired, \
       (u.status = 'active') AS user_active, \
       COALESCE((SELECT jsonb_agg(jsonb_build_array(m.organization_id::text, m.id::text, m.role) \
                                  ORDER BY m.id) \
                   FROM (SELECT organization_id, id, role FROM control.membership \
                          WHERE user_id = u.id AND status = 'active' ORDER BY id LIMIT 1001) m), \
                '[]'::jsonb) AS memberships \
  FROM identity.dashboard_session s JOIN identity.user u ON u.id = s.user_id \
 WHERE s.id = :credential_id";

/// Every key a region will accept right now.
pub const VERIFICATION_KEY_SET: &str = "\
SELECT kid, public_key, secret_ref, state, \
       (EXTRACT(EPOCH FROM retires_at)*1000)::bigint AS retires_at_ms \
  FROM control.signing_key \
 WHERE state IN ('active','retiring') \
 ORDER BY kid \
 LIMIT 8";

/// The one active signing key.
pub const ACTIVE_SIGNING_KEY: &str = "\
SELECT kid, public_key, secret_ref, state, \
       (EXTRACT(EPOCH FROM retires_at)*1000)::bigint AS retires_at_ms \
  FROM control.signing_key \
 WHERE state = 'active' \
 LIMIT 1";

/// The control pepper for one version.
///
/// `purpose` is projected as well as bound, so the row the keystore validates
/// the secret payload against carries every field the payload names. Trusting
/// the bound parameter instead would make the check compare a value to itself.
pub const CONTROL_PEPPER_BY_VERSION: &str = "\
SELECT version, purpose, state, secret_ref \
  FROM control.credential_pepper \
 WHERE purpose = :purpose AND version = :version";

/// The one active control pepper.
pub const ACTIVE_CONTROL_PEPPER: &str = "\
SELECT version, purpose, state, secret_ref \
  FROM control.credential_pepper \
 WHERE purpose = :purpose AND state = 'active'";

/// The coarse account state of one organization.
///
/// The `HTTP` edge runs this once per non-pause-exempt request, after the
/// authorization decision and never before it. It is driven from
/// `control.organization` rather than from `finance.account_state_v1` so that
/// the two absences stay distinguishable: an organization that does not exist
/// returns no row at all, while an organization whose finance row is missing
/// returns `'unavailable'` — and neither is ever `'active'`. Selecting from the
/// finance relation alone would collapse both into "no row" and tempt a caller
/// into treating it as a default.
pub const GET_ACCOUNT_STATE: &str = "\
SELECT COALESCE(a.status,'unavailable') AS account_status \
  FROM control.organization o \
  LEFT JOIN finance.account_state_v1 a ON a.organization_id = o.id \
 WHERE o.id = :organization_id";

/// Reads the lossless public account-state projection for a control response.
pub const GET_ACCOUNT_PROFILE: &str = "\
SELECT a.status, a.reason, a.revision, \
       (EXTRACT(EPOCH FROM a.changed_at)*1000)::bigint AS changed_at_ms, \
       COALESCE(e.epoch, 0) AS account_epoch \
  FROM finance.account_state_v1 a \
  LEFT JOIN control.authorization_epoch e \
    ON e.subject_kind = 'account' AND e.subject_id = a.organization_id \
 WHERE a.organization_id = :organization_id";

/// Reads one workspace's current revocation epoch.
pub const GET_WORKSPACE_EPOCH: &str = "\
SELECT COALESCE((SELECT e.epoch FROM control.authorization_epoch e \
                  WHERE e.subject_kind = 'workspace' AND e.subject_id = :workspace_id), 0)";

/// Resolves the in-flight replay row attached to one durable operation.
pub const GET_OPERATION_IDEMPOTENCY_ID: &str = "\
SELECT i.id FROM control.idempotency_record i \
 WHERE i.operation_id = :operation_id";

/// Reads one active caller role without trusting a requested organization id.
pub const GET_CALLER_ROLE: &str = "\
SELECT m.role FROM control.membership m \
 WHERE m.organization_id = :organization_id AND m.user_id = :user_id AND m.status = 'active'";

/// Reads the identity-owned current email projection.
pub const GET_USER_EMAIL: &str = "SELECT u.email FROM identity.user u WHERE u.id = :user_id";

/// Reads the tombstone instant needed by a successful public delete operation.
pub const GET_WORKSPACE_DELETED_AT: &str = "\
SELECT (EXTRACT(EPOCH FROM w.deleted_at)*1000)::bigint AS deleted_at_ms \
  FROM control.workspace w WHERE w.id = :workspace_id";

/// The readiness probe every control role runs at start-up.
pub const READINESS_PROBE: &str = "SELECT 1 AS ok";

/// The write probe `central-authz` runs at start-up and requires to **fail**.
///
/// A read-only role that can write is a misconfiguration, and the only safe
/// moment to discover it is before the first request rather than during one.
pub const AUTHZ_WRITE_PROBE: &str = "\
INSERT INTO control.audit_event \
  (id, actor_kind, action, resource_kind, outcome, request_id, occurred_at) \
VALUES (:id, 'system', 'authz.write_probe', 'user', 'denied', :request_id, \
        (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond'))";

/// Finds an idempotency record by its complete replay identity.
pub const FIND_IDEMPOTENCY: &str = "\
SELECT intent_hash, state, response_body, operation_id \
  FROM control.idempotency_record \
 WHERE key_kind = :key_kind AND key_value = :key_value \
   AND principal_kind = :principal_kind AND principal_id = :principal_id \
   AND scope_kind = :scope_kind AND scope_id = :scope_id \
   AND method = :method AND route = :route";

/// Inserts an in-flight replay record.
pub const INSERT_IDEMPOTENCY: &str = "\
INSERT INTO control.idempotency_record \
  (id, key_kind, key_value, principal_kind, principal_id, scope_kind, scope_id, \
   method, route, intent_hash, state, created_at, expires_at) \
VALUES (:id, :key_kind, :key_value, :principal_kind, :principal_id, :scope_kind, :scope_id, \
        :method, :route, :intent_hash, 'in_flight', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond')";

/// Completes an idempotency record with its replay body.
pub const COMPLETE_IDEMPOTENCY: &str = "\
UPDATE control.idempotency_record \
   SET state = 'completed', response_status = :response_status, response_body = :response_body, \
       operation_id = :operation_id, \
       completed_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :id AND state = 'in_flight'";

/// Attaches the durable operation while its replay record remains in flight.
pub const ATTACH_IDEMPOTENCY_OPERATION: &str = "\
UPDATE control.idempotency_record SET operation_id = :operation_id \
 WHERE id = :id AND state = 'in_flight' AND operation_id IS NULL";

/// Inserts an append-only audit event.
pub const INSERT_AUDIT: &str = "\
INSERT INTO control.audit_event \
  (id, organization_id, workspace_id, actor_kind, actor_id, action, resource_kind, resource_id, \
   outcome, request_id, operation_id, detail, occurred_at) \
VALUES (:id, :organization_id, :workspace_id, :actor_kind, :actor_id, :action, :resource_kind, \
        :resource_id, :outcome, :request_id, :operation_id, :detail, \
        TIMESTAMPTZ 'epoch' + :occurred_at_ms * INTERVAL '1 millisecond')";

/// Inserts one transactional-outbox message.
pub const INSERT_OUTBOX: &str = "\
INSERT INTO control.outbox_message \
  (id, topic, dedupe_key, group_key, payload, attempts, available_at, created_at) \
VALUES (:id, :topic, :dedupe_key, :group_key, :payload, :attempts, \
        TIMESTAMPTZ 'epoch' + :available_at_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :created_at_ms * INTERVAL '1 millisecond')";

/// Inserts an organization.
pub const INSERT_ORGANIZATION: &str = "\
INSERT INTO control.organization \
  (id, name, slug, status, revision, created_at, updated_at, created_by_user_id) \
VALUES (:id, :name, :slug, 'active', 1, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', :created_by_user_id)";

/// Inserts an active membership.
pub const INSERT_MEMBERSHIP: &str = "\
INSERT INTO control.membership \
  (id, organization_id, user_id, role, status, revision, created_at, updated_at) \
VALUES (:id, :organization_id, :user_id, :role, 'active', 1, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Ensures the finance account exists in the same transaction as its organization.
pub const ENSURE_FINANCE_ACCOUNT: &str =
    "SELECT finance.ensure_account(:organization_id) AS ensured";

/// Reads one organization.
pub const GET_ORGANIZATION: &str = "\
SELECT o.id, o.name, o.slug, o.status, o.revision, \
       (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM o.updated_at)*1000)::bigint AS updated_at_ms, o.created_by_user_id \
  FROM control.organization o WHERE o.id = :organization_id";

/// Lists an actor's organizations.
pub const LIST_ORGANIZATIONS: &str = "\
SELECT o.id, o.name, o.slug, o.status, o.revision, \
       (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM o.updated_at)*1000)::bigint AS updated_at_ms, o.created_by_user_id \
  FROM control.organization o JOIN control.membership m ON m.organization_id = o.id \
 WHERE m.user_id = :user_id AND m.status = 'active' \
   AND (:after_created_ms::bigint IS NULL \
    OR (o.created_at, o.id) > \
       (TIMESTAMPTZ 'epoch' + :after_created_ms * INTERVAL '1 millisecond', :after_id)) \
 ORDER BY o.created_at, o.id LIMIT :limit";

/// Lists current active memberships for one organization.
pub const LIST_MEMBERSHIPS: &str = "\
SELECT m.id, m.organization_id, m.user_id, m.role, m.status, m.revision, \
       (EXTRACT(EPOCH FROM m.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM m.updated_at)*1000)::bigint AS updated_at_ms \
  FROM control.membership m WHERE m.organization_id = :organization_id AND m.status = 'active' \
   AND (:after_created_ms::bigint IS NULL \
    OR (m.created_at, m.id) > \
       (TIMESTAMPTZ 'epoch' + :after_created_ms * INTERVAL '1 millisecond', :after_id)) \
 ORDER BY m.created_at, m.id LIMIT :limit";

/// Inserts an invitation with no secret column.
pub const INSERT_INVITATION: &str = "\
INSERT INTO control.invitation \
  (id, organization_id, email, role, status, invited_by_user_id, created_at, expires_at) \
VALUES (:id, :organization_id, :email, :role, 'pending', :invited_by_user_id, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond')";

/// Reads one invitation for a completed replay.
pub const GET_INVITATION: &str = "\
SELECT i.id, i.organization_id, i.email, i.role, i.status, i.invited_by_user_id, \
       i.accepted_user_id, \
       (EXTRACT(EPOCH FROM i.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM i.expires_at)*1000)::bigint AS expires_at_ms, \
       (EXTRACT(EPOCH FROM i.resolved_at)*1000)::bigint AS resolved_at_ms \
  FROM control.invitation i WHERE i.id = :invitation_id";

/// Reads invitations that a verified address may accept, under lock.
pub const FIND_ACCEPTABLE_INVITATIONS: &str = "\
SELECT i.id, i.organization_id, i.email, i.role, i.status, i.invited_by_user_id, \
       i.accepted_user_id, \
       (EXTRACT(EPOCH FROM i.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM i.expires_at)*1000)::bigint AS expires_at_ms, \
       (EXTRACT(EPOCH FROM i.resolved_at)*1000)::bigint AS resolved_at_ms \
  FROM control.invitation i \
 WHERE i.email = :email AND i.status = 'pending' \
   AND i.expires_at > TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 ORDER BY i.created_at, i.id LIMIT 100 FOR UPDATE";

/// Accepts one invitation.
pub const ACCEPT_INVITATION: &str = "\
UPDATE control.invitation SET status = 'accepted', accepted_user_id = :user_id, \
       resolved_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :invitation_id AND status = 'pending' \
   AND expires_at > TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond'";

/// Raises an existing membership to at least the invitation's role.
pub const FIND_MEMBERSHIP: &str = "\
SELECT m.id, m.organization_id, m.user_id, m.role, m.status, m.revision, \
       (EXTRACT(EPOCH FROM m.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM m.updated_at)*1000)::bigint AS updated_at_ms \
  FROM control.membership m \
 WHERE m.organization_id = :organization_id AND m.user_id = :user_id AND m.status = 'active'";

/// Raises but never lowers an active membership.
pub const RAISE_MEMBERSHIP_ROLE: &str = "\
UPDATE control.membership SET role = :role, revision = revision + 1, \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :membership_id AND status = 'active' \
   AND CASE role WHEN 'member' THEN 1 WHEN 'admin' THEN 2 ELSE 3 END \
       < CASE :role WHEN 'member' THEN 1 WHEN 'admin' THEN 2 ELSE 3 END";

/// Inserts the hidden central half of a workspace.
pub const INSERT_WORKSPACE: &str = "\
INSERT INTO control.workspace \
  (id, organization_id, name, slug, region, status, provision_operation_id, provision_fence, \
   revision, created_at, updated_at, created_by_user_id) \
VALUES (:id, :organization_id, :name, :slug, :region, 'provisioning', :operation_id, 1, 1, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', :created_by_user_id)";

/// Inserts a central durable operation.
pub const INSERT_OPERATION: &str = "\
INSERT INTO control.durable_operation \
  (id, kind, visibility, organization_id, workspace_id, principal_kind, principal_id, scopes, \
   status, intent_hash, fence, attempt, created_at, updated_at, due_at) \
VALUES (:id, :kind, :visibility, :organization_id, :workspace_id, :principal_kind, :principal_id, \
        :scopes, 'queued', :intent_hash, 1, 0, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Reads one workspace, including tombstones.
pub const GET_WORKSPACE: &str = "\
SELECT w.id, w.organization_id, w.name, w.slug, w.region, w.status, \
       w.provision_operation_id, w.provision_fence, w.deletion_operation_id, w.deletion_fence, \
       w.revision, (EXTRACT(EPOCH FROM w.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM w.updated_at)*1000)::bigint AS updated_at_ms, \
       (EXTRACT(EPOCH FROM w.activated_at)*1000)::bigint AS activated_at_ms, \
       (EXTRACT(EPOCH FROM w.deleted_at)*1000)::bigint AS deleted_at_ms, w.created_by_user_id \
  FROM control.workspace w WHERE w.id = :workspace_id";

/// Lists only publicly visible workspaces.
pub const LIST_WORKSPACES: &str = "\
SELECT w.id, w.organization_id, w.name, w.slug, w.region, w.status, \
       w.provision_operation_id, w.provision_fence, w.deletion_operation_id, w.deletion_fence, \
       w.revision, (EXTRACT(EPOCH FROM w.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM w.updated_at)*1000)::bigint AS updated_at_ms, \
       (EXTRACT(EPOCH FROM w.activated_at)*1000)::bigint AS activated_at_ms, \
       (EXTRACT(EPOCH FROM w.deleted_at)*1000)::bigint AS deleted_at_ms, w.created_by_user_id \
  FROM control.workspace w JOIN control.membership m ON m.organization_id = w.organization_id \
 WHERE m.user_id = :user_id AND m.status = 'active' AND w.status <> 'provisioning' \
   AND (:organization_id::uuid IS NULL OR w.organization_id = :organization_id) \
   AND (:after_created_ms::bigint IS NULL \
    OR (w.created_at, w.id) > \
       (TIMESTAMPTZ 'epoch' + :after_created_ms * INTERVAL '1 millisecond', :after_id)) \
 ORDER BY w.created_at, w.id LIMIT :limit";

/// Marks both workspace halves durable under the accepted fence.
pub const FINISH_WORKSPACE_PROVISION: &str = "\
UPDATE control.workspace SET status = 'active', provision_fence = :fence, revision = revision + 1, \
       activated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :workspace_id AND provision_operation_id = :operation_id \
   AND status = 'provisioning' AND provision_fence <= :fence";

/// Finishes a durable operation successfully under its fence.
pub const SUCCEED_OPERATION: &str = "\
UPDATE control.durable_operation SET status = 'succeeded', result = :result, lease_owner = NULL, \
       lease_expires_at = NULL, terminal_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', due_at = NULL \
 WHERE id = :operation_id AND status IN ('queued','running') AND fence = :fence";

/// Accepts workspace deletion and records its public operation.
pub const BEGIN_WORKSPACE_DELETION: &str = "\
UPDATE control.workspace SET status = 'deleting', deletion_operation_id = :operation_id, \
       deletion_fence = 1, revision = revision + 1, \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :workspace_id AND organization_id = :organization_id AND status = 'active'";

/// Revokes every key before deletion acceptance becomes visible.
pub const REVOKE_WORKSPACE_KEYS: &str = "\
UPDATE control.api_key SET revoked_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       revision = revision + 1 WHERE workspace_id = :workspace_id AND revoked_at IS NULL \
RETURNING id";

/// Advances one workspace or key epoch through its kind-specific wrapper.
pub const BUMP_WORKSPACE_EPOCH: &str =
    "SELECT control.bump_workspace_epoch(:workspace_id) AS epoch";
/// Advances one API-key epoch through its kind-specific wrapper.
pub const BUMP_KEY_EPOCH: &str = "SELECT control.bump_key_epoch(:key_id) AS epoch";

/// Completes a workspace tombstone under the worker's current fence.
pub const COMPLETE_WORKSPACE_DELETION: &str = "\
UPDATE control.workspace SET status = 'deleted', deletion_fence = :fence, revision = revision + 1, \
       deleted_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :workspace_id AND deletion_operation_id = :operation_id \
   AND status = 'deleting' AND deletion_fence <= :fence";

/// Inserts a workspace API key.
pub const INSERT_API_KEY: &str = "\
INSERT INTO control.api_key \
  (id, workspace_id, organization_id, name, scopes, region, verifier, pepper_version, \
   created_at, revision, created_by_user_id) \
VALUES (:id, :workspace_id, :organization_id, :name, :scopes, :region, :verifier, :pepper_version, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', 1, :created_by_user_id)";

/// Reads one API key without exposing its verifier.
pub const GET_API_KEY: &str = "\
SELECT k.id, k.workspace_id, k.organization_id, k.name, k.scopes, k.region, k.pepper_version, \
       (EXTRACT(EPOCH FROM k.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM k.revoked_at)*1000)::bigint AS revoked_at_ms, \
       k.revision, k.created_by_user_id FROM control.api_key k WHERE k.id = :key_id";

/// Lists a workspace's keys.
pub const LIST_API_KEYS: &str = "\
SELECT k.id, k.workspace_id, k.organization_id, k.name, k.scopes, k.region, k.pepper_version, \
       (EXTRACT(EPOCH FROM k.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM k.revoked_at)*1000)::bigint AS revoked_at_ms, \
       k.revision, k.created_by_user_id FROM control.api_key k \
 WHERE k.workspace_id = :workspace_id \
   AND (:after_created_ms::bigint IS NULL \
    OR (k.created_at, k.id) > \
       (TIMESTAMPTZ 'epoch' + :after_created_ms * INTERVAL '1 millisecond', :after_id)) \
 ORDER BY k.created_at, k.id LIMIT :limit";

/// Revokes one API key and honors `If-Match` when supplied.
pub const REVOKE_API_KEY: &str = "\
UPDATE control.api_key SET revoked_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       revision = revision + 1 WHERE id = :key_id AND workspace_id = :workspace_id \
   AND revoked_at IS NULL AND (:expected_revision::bigint IS NULL OR revision = :expected_revision)";

/// Reads one durable operation.
pub const GET_OPERATION: &str = "\
SELECT o.id, o.kind, o.visibility, o.organization_id, o.workspace_id, o.principal_id, o.scopes, \
       o.status, o.intent_hash, o.fence, o.attempt, o.lease_owner, \
       (EXTRACT(EPOCH FROM o.lease_expires_at)*1000)::bigint AS lease_expires_at_ms, \
       (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM o.started_at)*1000)::bigint AS started_at_ms, \
       (EXTRACT(EPOCH FROM o.updated_at)*1000)::bigint AS updated_at_ms, \
       (EXTRACT(EPOCH FROM o.terminal_at)*1000)::bigint AS terminal_at_ms, \
       (EXTRACT(EPOCH FROM o.due_at)*1000)::bigint AS due_at_ms \
  FROM control.durable_operation o WHERE o.id = :operation_id";

/// Lists public operations in one organization.
pub const LIST_OPERATIONS: &str = "\
SELECT o.id, o.kind, o.visibility, o.organization_id, o.workspace_id, o.principal_id, o.scopes, \
       o.status, o.intent_hash, o.fence, o.attempt, o.lease_owner, \
       (EXTRACT(EPOCH FROM o.lease_expires_at)*1000)::bigint AS lease_expires_at_ms, \
       (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM o.started_at)*1000)::bigint AS started_at_ms, \
       (EXTRACT(EPOCH FROM o.updated_at)*1000)::bigint AS updated_at_ms, \
       (EXTRACT(EPOCH FROM o.terminal_at)*1000)::bigint AS terminal_at_ms, \
       (EXTRACT(EPOCH FROM o.due_at)*1000)::bigint AS due_at_ms \
  FROM control.durable_operation o \
 WHERE o.organization_id = :organization_id AND o.visibility = 'public' \
   AND (:kind::text IS NULL OR o.kind = :kind) \
   AND (:status::text IS NULL OR o.status = :status) \
   AND (:after_created_ms::bigint IS NULL \
    OR (o.created_at, o.id) > \
       (TIMESTAMPTZ 'epoch' + :after_created_ms * INTERVAL '1 millisecond', :after_id)) \
 ORDER BY o.created_at, o.id LIMIT :limit";

/// Claims a bounded batch of due operations with skip-locked exclusivity.
pub const CLAIM_DUE_OPERATIONS: &str = "\
WITH due AS (SELECT id FROM control.durable_operation \
 WHERE status IN ('queued','running') AND attempt < 100 \
   AND (due_at IS NULL OR due_at <= TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 ORDER BY due_at NULLS FIRST, id LIMIT :batch FOR UPDATE SKIP LOCKED) \
UPDATE control.durable_operation o SET status = 'running', fence = fence + 1, attempt = attempt + 1, \
       lease_owner = :owner, lease_expires_at = TIMESTAMPTZ 'epoch' + :lease_expires_at_ms * INTERVAL '1 millisecond', \
       started_at = COALESCE(started_at, TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond'), \
       updated_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       due_at = TIMESTAMPTZ 'epoch' + :lease_expires_at_ms * INTERVAL '1 millisecond' \
  FROM due WHERE o.id = due.id \
RETURNING o.id, o.kind, o.visibility, o.organization_id, o.workspace_id, o.principal_id, o.scopes, \
 o.status, o.intent_hash, o.fence, o.attempt, o.lease_owner, \
 (EXTRACT(EPOCH FROM o.lease_expires_at)*1000)::bigint, \
 (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint, (EXTRACT(EPOCH FROM o.started_at)*1000)::bigint, \
 (EXTRACT(EPOCH FROM o.updated_at)*1000)::bigint, (EXTRACT(EPOCH FROM o.terminal_at)*1000)::bigint, \
 (EXTRACT(EPOCH FROM o.due_at)*1000)::bigint";

/// Claims a bounded outbox batch with skip-locked exclusivity.
pub const CLAIM_OUTBOX: &str = "\
WITH due AS (SELECT id FROM control.outbox_message \
 WHERE dispatched_at IS NULL AND attempts < 100 \
   AND available_at <= TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
   AND (claimed_until IS NULL OR claimed_until <= TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 ORDER BY available_at, id LIMIT :batch FOR UPDATE SKIP LOCKED) \
UPDATE control.outbox_message o SET attempts = attempts + 1, claimed_by = :owner, \
       claimed_until = TIMESTAMPTZ 'epoch' + :lease_expires_at_ms * INTERVAL '1 millisecond' \
  FROM due WHERE o.id = due.id \
RETURNING o.id, o.topic, o.dedupe_key, o.group_key, o.payload, o.attempts, \
 (EXTRACT(EPOCH FROM o.available_at)*1000)::bigint, o.claimed_by, \
 (EXTRACT(EPOCH FROM o.claimed_until)*1000)::bigint, \
 (EXTRACT(EPOCH FROM o.dispatched_at)*1000)::bigint, o.last_error, \
 (EXTRACT(EPOCH FROM o.created_at)*1000)::bigint";

/// Marks one claimed message dispatched.
pub const MARK_OUTBOX_DISPATCHED: &str = "\
UPDATE control.outbox_message SET dispatched_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
       claimed_by = NULL, claimed_until = NULL, last_error = NULL \
 WHERE id = :id AND dispatched_at IS NULL";

/// Releases an outbox claim after a redacted failure.
pub const RELEASE_OUTBOX: &str = "\
UPDATE control.outbox_message SET available_at = TIMESTAMPTZ 'epoch' + :available_at_ms * INTERVAL '1 millisecond', \
       claimed_by = NULL, claimed_until = NULL, last_error = :last_error \
 WHERE id = :id AND dispatched_at IS NULL";

/// Sweeps bounded expired replay rows.
pub const GC_IDEMPOTENCY: &str = "\
DELETE FROM control.idempotency_record WHERE id IN \
 (SELECT id FROM control.idempotency_record \
   WHERE expires_at <= TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
   ORDER BY expires_at, id LIMIT :batch)";

/// Sweeps bounded dispatched outbox rows.
pub const GC_OUTBOX: &str = "\
DELETE FROM control.outbox_message WHERE id IN \
 (SELECT id FROM control.outbox_message WHERE dispatched_at IS NOT NULL \
   ORDER BY dispatched_at, id LIMIT :batch)";

/// Every statement this crate issues, for the discipline scan.
pub const ALL: &[(&str, &str)] = &[
    ("RESOLVE_WORKSPACE_KEY", RESOLVE_WORKSPACE_KEY),
    (
        "RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE",
        RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE,
    ),
    (
        "RESOLVE_SESSION_FOR_WORKSPACE",
        RESOLVE_SESSION_FOR_WORKSPACE,
    ),
    (
        "RESOLVE_ACCOUNT_TOKEN_CENTRAL",
        RESOLVE_ACCOUNT_TOKEN_CENTRAL,
    ),
    ("RESOLVE_SESSION_CENTRAL", RESOLVE_SESSION_CENTRAL),
    ("VERIFICATION_KEY_SET", VERIFICATION_KEY_SET),
    ("ACTIVE_SIGNING_KEY", ACTIVE_SIGNING_KEY),
    ("CONTROL_PEPPER_BY_VERSION", CONTROL_PEPPER_BY_VERSION),
    ("ACTIVE_CONTROL_PEPPER", ACTIVE_CONTROL_PEPPER),
    ("GET_ACCOUNT_STATE", GET_ACCOUNT_STATE),
    ("GET_ACCOUNT_PROFILE", GET_ACCOUNT_PROFILE),
    ("GET_WORKSPACE_EPOCH", GET_WORKSPACE_EPOCH),
    ("GET_OPERATION_IDEMPOTENCY_ID", GET_OPERATION_IDEMPOTENCY_ID),
    ("GET_CALLER_ROLE", GET_CALLER_ROLE),
    ("GET_USER_EMAIL", GET_USER_EMAIL),
    ("GET_WORKSPACE_DELETED_AT", GET_WORKSPACE_DELETED_AT),
    ("READINESS_PROBE", READINESS_PROBE),
    ("AUTHZ_WRITE_PROBE", AUTHZ_WRITE_PROBE),
    ("FIND_IDEMPOTENCY", FIND_IDEMPOTENCY),
    ("INSERT_IDEMPOTENCY", INSERT_IDEMPOTENCY),
    ("COMPLETE_IDEMPOTENCY", COMPLETE_IDEMPOTENCY),
    ("ATTACH_IDEMPOTENCY_OPERATION", ATTACH_IDEMPOTENCY_OPERATION),
    ("INSERT_AUDIT", INSERT_AUDIT),
    ("INSERT_OUTBOX", INSERT_OUTBOX),
    ("INSERT_ORGANIZATION", INSERT_ORGANIZATION),
    ("INSERT_MEMBERSHIP", INSERT_MEMBERSHIP),
    ("ENSURE_FINANCE_ACCOUNT", ENSURE_FINANCE_ACCOUNT),
    ("GET_ORGANIZATION", GET_ORGANIZATION),
    ("LIST_ORGANIZATIONS", LIST_ORGANIZATIONS),
    ("LIST_MEMBERSHIPS", LIST_MEMBERSHIPS),
    ("INSERT_INVITATION", INSERT_INVITATION),
    ("GET_INVITATION", GET_INVITATION),
    ("FIND_ACCEPTABLE_INVITATIONS", FIND_ACCEPTABLE_INVITATIONS),
    ("ACCEPT_INVITATION", ACCEPT_INVITATION),
    ("FIND_MEMBERSHIP", FIND_MEMBERSHIP),
    ("RAISE_MEMBERSHIP_ROLE", RAISE_MEMBERSHIP_ROLE),
    ("INSERT_WORKSPACE", INSERT_WORKSPACE),
    ("INSERT_OPERATION", INSERT_OPERATION),
    ("GET_WORKSPACE", GET_WORKSPACE),
    ("LIST_WORKSPACES", LIST_WORKSPACES),
    ("FINISH_WORKSPACE_PROVISION", FINISH_WORKSPACE_PROVISION),
    ("SUCCEED_OPERATION", SUCCEED_OPERATION),
    ("BEGIN_WORKSPACE_DELETION", BEGIN_WORKSPACE_DELETION),
    ("REVOKE_WORKSPACE_KEYS", REVOKE_WORKSPACE_KEYS),
    ("BUMP_WORKSPACE_EPOCH", BUMP_WORKSPACE_EPOCH),
    ("BUMP_KEY_EPOCH", BUMP_KEY_EPOCH),
    ("COMPLETE_WORKSPACE_DELETION", COMPLETE_WORKSPACE_DELETION),
    ("INSERT_API_KEY", INSERT_API_KEY),
    ("GET_API_KEY", GET_API_KEY),
    ("LIST_API_KEYS", LIST_API_KEYS),
    ("REVOKE_API_KEY", REVOKE_API_KEY),
    ("GET_OPERATION", GET_OPERATION),
    ("LIST_OPERATIONS", LIST_OPERATIONS),
    ("CLAIM_DUE_OPERATIONS", CLAIM_DUE_OPERATIONS),
    ("CLAIM_OUTBOX", CLAIM_OUTBOX),
    ("MARK_OUTBOX_DISPATCHED", MARK_OUTBOX_DISPATCHED),
    ("RELEASE_OUTBOX", RELEASE_OUTBOX),
    ("GC_IDEMPOTENCY", GC_IDEMPOTENCY),
    ("GC_OUTBOX", GC_OUTBOX),
];
