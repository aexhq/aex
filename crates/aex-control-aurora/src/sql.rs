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
pub const CONTROL_PEPPER_BY_VERSION: &str = "\
SELECT version, secret_ref, state \
  FROM control.credential_pepper \
 WHERE purpose = :purpose AND version = :version";

/// The one active control pepper.
pub const ACTIVE_CONTROL_PEPPER: &str = "\
SELECT version, secret_ref, state \
  FROM control.credential_pepper \
 WHERE purpose = :purpose AND state = 'active'";

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
    ("VERIFICATION_KEY_SET", VERIFICATION_KEY_SET),
    ("ACTIVE_SIGNING_KEY", ACTIVE_SIGNING_KEY),
    ("CONTROL_PEPPER_BY_VERSION", CONTROL_PEPPER_BY_VERSION),
    ("ACTIVE_CONTROL_PEPPER", ACTIVE_CONTROL_PEPPER),
    ("READINESS_PROBE", READINESS_PROBE),
    ("AUTHZ_WRITE_PROBE", AUTHZ_WRITE_PROBE),
];
