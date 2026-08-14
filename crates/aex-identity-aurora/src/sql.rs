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

/// Set a person's status, conditionally on it actually changing.
pub const SET_USER_STATUS: &str = "\
UPDATE identity.user \
   SET status = :status, revision = revision + 1, \
       updated_at = (TIMESTAMPTZ 'epoch' + :now_ms * INTERVAL '1 millisecond') \
 WHERE id = :user_id AND status <> :status";

/// Advances the user epoch after a revoking transition.
pub const BUMP_USER_EPOCH: &str = "SELECT control.bump_user_epoch(:user_id) AS epoch";

/// The one active identity pepper, for a purpose.
///
/// The lifecycle row, never the material: `secret_ref` is a Secrets Manager
/// version id and the keystore fetches the bytes itself. The partial unique
/// index on `(purpose) WHERE state = 'active'` is what makes this at most one
/// row without an `ORDER BY` to pick a winner from.
pub const ACTIVE_IDENTITY_PEPPER: &str = "\
SELECT version, purpose, state, secret_ref \
  FROM identity.credential_pepper \
 WHERE purpose = :purpose AND state = 'active'";

/// One identity pepper version's lifecycle row.
///
/// Looked up by `(purpose, version)` because a credential names its version and
/// nothing else; resolving it any other way would verify against a pepper the
/// row does not name.
pub const IDENTITY_PEPPER_BY_VERSION: &str = "\
SELECT version, purpose, state, secret_ref \
  FROM identity.credential_pepper \
 WHERE purpose = :purpose AND version = :version";

/// Whether the caller is an active member of one organization.
///
/// The membership check is a **separate** statement from the account read below,
/// and deliberately so: "you are not a member" and "this organization has no
/// account row" are two different answers — `forbidden` and
/// `account_state_unavailable` — and one joined query could only tell a caller
/// that it got nothing.
pub const CALLER_ORGANIZATION_MEMBERSHIP: &str = "SELECT 1 AS ok   FROM control.membership  WHERE organization_id = :organization_id AND user_id = :user_id AND status = 'active'";

/// One organization's published account state.
///
/// `finance.account_state_v1` is the projection finance's own pause and resume
/// trigger writes, and it is the sole input to every published
/// `AccountOperationalState` on either plane. This statement reads it and
/// nothing else: no balance, no epoch, no derivation. A second producer of this
/// fact is how the same account came to be reported active by one route and
/// paused by another in the same second.
pub const ACCOUNT_OPERATIONAL_STATE: &str = "SELECT status, reason, revision, (EXTRACT(EPOCH FROM changed_at)*1000)::bigint AS changed_at_ms   FROM finance.account_state_v1  WHERE organization_id = :organization_id";

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
    ("SET_USER_STATUS", SET_USER_STATUS),
    ("BUMP_USER_EPOCH", BUMP_USER_EPOCH),
    ("ACTIVE_IDENTITY_PEPPER", ACTIVE_IDENTITY_PEPPER),
    ("IDENTITY_PEPPER_BY_VERSION", IDENTITY_PEPPER_BY_VERSION),
    (
        "CALLER_ORGANIZATION_MEMBERSHIP",
        CALLER_ORGANIZATION_MEMBERSHIP,
    ),
    ("ACCOUNT_OPERATIONAL_STATE", ACCOUNT_OPERATIONAL_STATE),
    ("READINESS_PROBE", READINESS_PROBE),
];

/// The statements that consume a single-use credential.
///
/// Every one of them must carry its own guard in the `WHERE` clause, which
/// `statements.rs` asserts.
pub const SINGLE_USE: &[(&str, &str)] = &[
    ("CONSUME_EMAIL_CHALLENGE", CONSUME_EMAIL_CHALLENGE),
    ("REVOKE_DASHBOARD_SESSION", REVOKE_DASHBOARD_SESSION),
];
