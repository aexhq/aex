//! Every identity statement, as a `const &str`.
//!
//! The single-use credentials share one shape: a keyed lookup by primary key,
//! then a **conditional** `UPDATE` whose predicate repeats the domain guard.
//! The guard is in the predicate rather than in the application because two
//! valid concurrent consumers must yield exactly one winner, and an application
//! that read-then-wrote would have to arbitrate a race it cannot see.
//!
//! A wrong secret never reaches the `UPDATE`: the verifier is compared in Rust,
//! in constant time, before the statement is issued. That is what stops a
//! guessing attacker from burning a live challenge.

/// Finds the person already linked to a provider account.
pub const FIND_USER_BY_EXTERNAL_IDENTITY: &str = "\
SELECT u.id, u.email, \
       (EXTRACT(EPOCH FROM u.email_verified_at)*1000)::bigint AS email_verified_at_ms, \
       u.name, u.image_url, u.status, u.revision, \
       (EXTRACT(EPOCH FROM u.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM u.updated_at)*1000)::bigint AS updated_at_ms \
  FROM identity.external_identity e \
  JOIN identity.user u ON u.id = e.user_id \
 WHERE e.provider = :provider AND e.provider_account_id = :provider_account_id";

/// Finds a person by their normalized email.
pub const FIND_USER_BY_EMAIL: &str = "\
SELECT u.id, u.email, \
       (EXTRACT(EPOCH FROM u.email_verified_at)*1000)::bigint AS email_verified_at_ms, \
       u.name, u.image_url, u.status, u.revision, \
       (EXTRACT(EPOCH FROM u.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM u.updated_at)*1000)::bigint AS updated_at_ms \
  FROM identity.user u WHERE u.email = :email";

/// Finds one person by id.
pub const FIND_USER_BY_ID: &str = "\
SELECT u.id, u.email, \
       (EXTRACT(EPOCH FROM u.email_verified_at)*1000)::bigint AS email_verified_at_ms, \
       u.name, u.image_url, u.status, u.revision, \
       (EXTRACT(EPOCH FROM u.created_at)*1000)::bigint AS created_at_ms, \
       (EXTRACT(EPOCH FROM u.updated_at)*1000)::bigint AS updated_at_ms \
  FROM identity.user u WHERE u.id = :user_id";

/// Inserts a person with a preassigned identity.
pub const INSERT_USER: &str = "\
INSERT INTO identity.user \
  (id, email, email_verified_at, name, image_url, status, revision, created_at, updated_at) \
VALUES (:id, :email, \
        CASE WHEN :email_verified THEN TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' END, \
        :name, :image_url, 'active', 1, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Adds a provider link to a person.
pub const INSERT_EXTERNAL_IDENTITY: &str = "\
INSERT INTO identity.external_identity \
  (id, user_id, provider, provider_account_id, linked_at) \
VALUES (:id, :user_id, :provider, :provider_account_id, \
        TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Lists every provider link for one person in stable identity order.
pub const LIST_EXTERNAL_IDENTITIES: &str = "\
SELECT e.id, e.user_id, e.provider, e.provider_account_id, \
       (EXTRACT(EPOCH FROM e.linked_at)*1000)::bigint AS linked_at_ms \
  FROM identity.external_identity e \
 WHERE e.user_id = :user_id ORDER BY e.id LIMIT 16";

/// Inserts a single-use email challenge.
pub const INSERT_EMAIL_CHALLENGE: &str = "\
INSERT INTO identity.email_challenge \
  (id, email, verifier, pepper_version, purpose, issued_at, expires_at) \
VALUES (:id, :email, :verifier, :pepper_version, 'sign_in', \
        TIMESTAMPTZ 'epoch' + :issued_at_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond')";

/// Resolve a credential row by its primary key.
///
/// The lookup is by id rather than by verifier, so verification survives a
/// pepper rotation and a forged id costs one primary-key miss.
pub const RESOLVE_EMAIL_CHALLENGE: &str = "\
SELECT c.id, c.email, c.verifier, c.pepper_version, \
       (EXTRACT(EPOCH FROM c.issued_at)*1000)::bigint AS issued_at_ms, \
       (EXTRACT(EPOCH FROM c.expires_at)*1000)::bigint AS expires_at_ms, \
       (EXTRACT(EPOCH FROM c.consumed_at)*1000)::bigint AS consumed_at_ms \
  FROM identity.email_challenge c \
 WHERE c.id = :challenge_id";

/// Redeem a sign-in link, conditionally.
///
/// `consumed_at IS NULL AND expires_at > now` is the whole race arbiter: N
/// concurrent consumers of a live challenge produce exactly one affected row.
pub const CONSUME_EMAIL_CHALLENGE: &str = "\
UPDATE identity.email_challenge \
   SET consumed_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :challenge_id \
   AND consumed_at IS NULL \
   AND expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Resolve a browser session and its person, for the actor read.
///
/// This statement performs no write. `aex_authz` holds no write privilege
/// anywhere, which is what proves it rather than a comment.
pub const RESOLVE_DASHBOARD_SESSION: &str = "\
SELECT s.id, s.user_id, s.verifier, s.pepper_version, \
       (EXTRACT(EPOCH FROM s.issued_at)*1000)::bigint AS issued_at_ms, \
       (EXTRACT(EPOCH FROM s.expires_at)*1000)::bigint AS expires_at_ms, \
       (EXTRACT(EPOCH FROM s.revoked_at)*1000)::bigint AS revoked_at_ms, \
       u.email, (EXTRACT(EPOCH FROM u.email_verified_at)*1000)::bigint AS email_verified_at_ms, \
       u.name, u.image_url, u.status, u.revision, \
       (EXTRACT(EPOCH FROM u.created_at)*1000)::bigint AS user_created_at_ms, \
       (EXTRACT(EPOCH FROM u.updated_at)*1000)::bigint AS user_updated_at_ms \
  FROM identity.dashboard_session s \
  JOIN identity.user u ON u.id = s.user_id \
 WHERE s.id = :session_id";

/// Inserts a dashboard session only for a currently active person.
pub const INSERT_DASHBOARD_SESSION: &str = "\
INSERT INTO identity.dashboard_session \
  (id, user_id, verifier, pepper_version, issued_at, expires_at) \
SELECT :id, u.id, :verifier, :pepper_version, \
       TIMESTAMPTZ 'epoch' + :issued_at_ms * INTERVAL '1 millisecond', \
       TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond' \
  FROM identity.user u WHERE u.id = :user_id AND u.status = 'active'";

/// Close a browser session, conditionally.
pub const REVOKE_DASHBOARD_SESSION: &str = "\
UPDATE identity.dashboard_session \
   SET revoked_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :session_id AND revoked_at IS NULL";

/// Removes a provider link only when another sign-in path remains.
pub const UNLINK_EXTERNAL_IDENTITY: &str = "\
DELETE FROM identity.external_identity e \
 WHERE e.user_id = :user_id AND e.provider = :provider \
   AND (EXISTS (SELECT 1 FROM identity.external_identity other \
                 WHERE other.user_id = e.user_id AND other.provider <> e.provider) \
        OR EXISTS (SELECT 1 FROM identity.user u \
                    WHERE u.id = e.user_id AND u.email_verified_at IS NOT NULL))";

/// Inserts a device authorization.
pub const INSERT_DEVICE_AUTHORIZATION: &str = "\
INSERT INTO identity.device_authorization \
  (id, device_verifier, user_code_hash, pepper_version, status, requested_scopes, \
   issued_at, expires_at, poll_interval_ms) \
VALUES (:id, :device_verifier, :user_code_hash, :pepper_version, 'pending', :requested_scopes, \
        TIMESTAMPTZ 'epoch' + :issued_at_ms * INTERVAL '1 millisecond', \
        TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond', :poll_interval_ms)";

/// Persists the next poll instant and any slow-down interval.
pub const UPDATE_DEVICE_POLL: &str = "\
UPDATE identity.device_authorization \
   SET poll_interval_ms = :poll_interval_ms, \
       last_polled_at = TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond' \
 WHERE id = :device_id AND status IN ('pending','approved')";

/// Reads one account token after a mutation or replay.
pub const RESOLVE_ACCOUNT_TOKEN: &str = "\
SELECT t.id, t.user_id, t.name, t.scopes, t.origin, t.pepper_version, \
       (EXTRACT(EPOCH FROM t.issued_at)*1000)::bigint AS issued_at_ms, \
       (EXTRACT(EPOCH FROM t.expires_at)*1000)::bigint AS expires_at_ms, \
       (EXTRACT(EPOCH FROM t.revoked_at)*1000)::bigint AS revoked_at_ms \
  FROM identity.account_token t WHERE t.id = :token_id";

/// Resolve a device authorization by its device-code id.
pub const RESOLVE_DEVICE_AUTHORIZATION: &str = "\
SELECT d.id, d.device_verifier, d.pepper_version, d.status, d.requested_scopes, \
       d.approved_by_user_id, \
       (EXTRACT(EPOCH FROM d.approved_at)*1000)::bigint AS approved_at_ms, \
       (EXTRACT(EPOCH FROM d.consumed_at)*1000)::bigint AS consumed_at_ms, \
       d.account_token_id, \
       (EXTRACT(EPOCH FROM d.issued_at)*1000)::bigint AS issued_at_ms, \
       (EXTRACT(EPOCH FROM d.expires_at)*1000)::bigint AS expires_at_ms, \
       d.poll_interval_ms, \
       (EXTRACT(EPOCH FROM d.last_polled_at)*1000)::bigint AS last_polled_at_ms \
  FROM identity.device_authorization d \
 WHERE d.id = :device_id";

/// Resolves a device authorization by the keyed human-code digest.
pub const RESOLVE_DEVICE_BY_USER_CODE: &str = "\
SELECT d.id, d.device_verifier, d.pepper_version, d.status, d.requested_scopes, \
       d.approved_by_user_id, \
       (EXTRACT(EPOCH FROM d.approved_at)*1000)::bigint AS approved_at_ms, \
       (EXTRACT(EPOCH FROM d.consumed_at)*1000)::bigint AS consumed_at_ms, \
       d.account_token_id, \
       (EXTRACT(EPOCH FROM d.issued_at)*1000)::bigint AS issued_at_ms, \
       (EXTRACT(EPOCH FROM d.expires_at)*1000)::bigint AS expires_at_ms, \
       d.poll_interval_ms, \
       (EXTRACT(EPOCH FROM d.last_polled_at)*1000)::bigint AS last_polled_at_ms \
  FROM identity.device_authorization d \
 WHERE d.user_code_hash = :user_code_hash";

/// Approve a device authorization, conditionally, by its keyed user-code digest.
///
/// The approver's currency is part of the predicate rather than a prior read:
/// an `EXISTS` over a live session belonging to an active person makes
/// "only a current dashboard actor may approve" a property of the write.
pub const APPROVE_DEVICE_AUTHORIZATION: &str = "\
UPDATE identity.device_authorization d \
   SET status = 'approved', \
       approved_by_user_id = :actor_user_id, \
       approved_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE d.user_code_hash = :user_code_hash \
   AND d.status = 'pending' \
   AND d.expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
   AND EXISTS (SELECT 1 FROM identity.dashboard_session s \
                JOIN identity.user u ON u.id = s.user_id \
               WHERE s.id = :actor_session_id \
                 AND s.user_id = :actor_user_id \
                 AND s.revoked_at IS NULL \
                 AND s.expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
                 AND u.status = 'active')";

/// Refuse a device authorization, conditionally.
pub const DENY_DEVICE_AUTHORIZATION: &str = "\
UPDATE identity.device_authorization d \
   SET status = 'denied' \
 WHERE d.user_code_hash = :user_code_hash \
   AND d.status IN ('pending','approved') \
   AND d.expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Redeem an approved device grant, conditionally.
///
/// Runs in the same transaction as the account-token insert. A grant consumed
/// without a token is a credential the caller can never obtain; a token without
/// a consumed grant is one they could obtain twice.
pub const CONSUME_DEVICE_AUTHORIZATION: &str = "\
UPDATE identity.device_authorization \
   SET status = 'consumed', \
       account_token_id = :token_id, \
       consumed_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :device_id \
   AND status = 'approved' \
   AND expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Insert the account token a redeemed grant mints.
pub const INSERT_ACCOUNT_TOKEN: &str = "\
INSERT INTO identity.account_token \
  (id, user_id, verifier, pepper_version, name, scopes, origin, issued_at, expires_at) \
VALUES (:id, :user_id, :verifier, :pepper_version, :name, :scopes, 'device_flow', \
        (TIMESTAMPTZ 'epoch' + :issued_at_ms * INTERVAL '1 millisecond'), \
        (TIMESTAMPTZ 'epoch' + :expires_at_ms * INTERVAL '1 millisecond'))";

/// Revoke an account token, conditionally.
pub const REVOKE_ACCOUNT_TOKEN: &str = "\
UPDATE identity.account_token \
   SET revoked_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :token_id AND user_id = :user_id AND revoked_at IS NULL \
   AND expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// How many live tokens a person still holds.
///
/// Read in the same transaction as a revocation, so the `user` epoch advances
/// exactly when the last one goes.
pub const COUNT_LIVE_ACCOUNT_TOKENS: &str = "\
SELECT count(*) AS live \
  FROM identity.account_token \
 WHERE user_id = :user_id AND revoked_at IS NULL \
   AND expires_at > (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond')";

/// Set a person's status, conditionally on it actually changing.
pub const SET_USER_STATUS: &str = "\
UPDATE identity.user \
   SET status = :status, revision = revision + 1, \
       updated_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :user_id AND status <> :status";

/// Advances the user epoch after a revoking transition.
pub const BUMP_USER_EPOCH: &str = "SELECT control.bump_user_epoch(:user_id) AS epoch";

/// The identity readiness probe.
pub const READINESS_PROBE: &str = "SELECT 1 AS ok";

/// Every statement this crate issues, for the discipline scan.
pub const ALL: &[(&str, &str)] = &[
    (
        "FIND_USER_BY_EXTERNAL_IDENTITY",
        FIND_USER_BY_EXTERNAL_IDENTITY,
    ),
    ("FIND_USER_BY_EMAIL", FIND_USER_BY_EMAIL),
    ("FIND_USER_BY_ID", FIND_USER_BY_ID),
    ("INSERT_USER", INSERT_USER),
    ("INSERT_EXTERNAL_IDENTITY", INSERT_EXTERNAL_IDENTITY),
    ("LIST_EXTERNAL_IDENTITIES", LIST_EXTERNAL_IDENTITIES),
    ("INSERT_EMAIL_CHALLENGE", INSERT_EMAIL_CHALLENGE),
    ("RESOLVE_EMAIL_CHALLENGE", RESOLVE_EMAIL_CHALLENGE),
    ("CONSUME_EMAIL_CHALLENGE", CONSUME_EMAIL_CHALLENGE),
    ("RESOLVE_DASHBOARD_SESSION", RESOLVE_DASHBOARD_SESSION),
    ("INSERT_DASHBOARD_SESSION", INSERT_DASHBOARD_SESSION),
    ("REVOKE_DASHBOARD_SESSION", REVOKE_DASHBOARD_SESSION),
    ("UNLINK_EXTERNAL_IDENTITY", UNLINK_EXTERNAL_IDENTITY),
    ("INSERT_DEVICE_AUTHORIZATION", INSERT_DEVICE_AUTHORIZATION),
    ("UPDATE_DEVICE_POLL", UPDATE_DEVICE_POLL),
    ("RESOLVE_DEVICE_AUTHORIZATION", RESOLVE_DEVICE_AUTHORIZATION),
    ("RESOLVE_DEVICE_BY_USER_CODE", RESOLVE_DEVICE_BY_USER_CODE),
    ("APPROVE_DEVICE_AUTHORIZATION", APPROVE_DEVICE_AUTHORIZATION),
    ("DENY_DEVICE_AUTHORIZATION", DENY_DEVICE_AUTHORIZATION),
    ("CONSUME_DEVICE_AUTHORIZATION", CONSUME_DEVICE_AUTHORIZATION),
    ("INSERT_ACCOUNT_TOKEN", INSERT_ACCOUNT_TOKEN),
    ("RESOLVE_ACCOUNT_TOKEN", RESOLVE_ACCOUNT_TOKEN),
    ("REVOKE_ACCOUNT_TOKEN", REVOKE_ACCOUNT_TOKEN),
    ("COUNT_LIVE_ACCOUNT_TOKENS", COUNT_LIVE_ACCOUNT_TOKENS),
    ("SET_USER_STATUS", SET_USER_STATUS),
    ("BUMP_USER_EPOCH", BUMP_USER_EPOCH),
    ("READINESS_PROBE", READINESS_PROBE),
];

/// The statements that consume a single-use credential.
///
/// Every one of them must carry its own guard in the `WHERE` clause, which
/// `statements.rs` asserts.
pub const SINGLE_USE: &[(&str, &str)] = &[
    ("CONSUME_EMAIL_CHALLENGE", CONSUME_EMAIL_CHALLENGE),
    ("APPROVE_DEVICE_AUTHORIZATION", APPROVE_DEVICE_AUTHORIZATION),
    ("DENY_DEVICE_AUTHORIZATION", DENY_DEVICE_AUTHORIZATION),
    ("CONSUME_DEVICE_AUTHORIZATION", CONSUME_DEVICE_AUTHORIZATION),
    ("REVOKE_DASHBOARD_SESSION", REVOKE_DASHBOARD_SESSION),
    ("REVOKE_ACCOUNT_TOKEN", REVOKE_ACCOUNT_TOKEN),
];
