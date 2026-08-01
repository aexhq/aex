-- 0002_identity.sql — people, provider links, and the four AEX-minted
-- identity credentials.
--
-- Every credential row stores a 32-byte keyed verifier plus the explicit pepper
-- version it was computed under, and nothing else derived from the secret. A
-- pepper retires only once `identity.credential_pepper_usage` reports zero live
-- references, so losing one invalidates exactly its counted rows rather than an
-- unknown set.
--
-- `identity.external_identity` deliberately has no access-token, refresh-token
-- or id-token column. AEX needs no provider API access, and a schema test
-- asserts their absence so reintroducing one is a failing test rather than a
-- review comment.

CREATE SCHEMA identity;

CREATE TABLE identity.credential_pepper (
  version     smallint PRIMARY KEY,
  purpose     text NOT NULL,
  state       text NOT NULL,
  secret_ref  text NOT NULL,
  created_at  timestamptz NOT NULL,
  retired_at  timestamptz,
  CONSTRAINT pepper_purpose_ck CHECK (purpose IN ('identity','cursor')),
  CONSTRAINT pepper_state_ck   CHECK (state IN ('active','retiring','retired')),
  CONSTRAINT pepper_retired_ck CHECK ((state = 'retired') = (retired_at IS NOT NULL)));
CREATE UNIQUE INDEX pepper_active_uk
  ON identity.credential_pepper (purpose) WHERE state = 'active';

CREATE TABLE identity.user (
  id                uuid PRIMARY KEY,
  email             text NOT NULL,
  email_verified_at timestamptz,
  name              text,
  image_url         text,
  status            text NOT NULL DEFAULT 'active',
  revision          bigint NOT NULL DEFAULT 1,
  created_at        timestamptz NOT NULL,
  updated_at        timestamptz NOT NULL,
  CONSTRAINT user_status_ck     CHECK (status IN ('active','disabled')),
  CONSTRAINT user_email_lower_ck CHECK (email = lower(email)),
  CONSTRAINT user_email_len_ck  CHECK (char_length(email) BETWEEN 3 AND 254),
  CONSTRAINT user_email_at_ck   CHECK (position('@' IN email) > 1),
  CONSTRAINT user_name_len_ck   CHECK (name IS NULL OR char_length(name) BETWEEN 1 AND 128),
  CONSTRAINT user_revision_ck   CHECK (revision >= 1));
CREATE UNIQUE INDEX user_email_uk ON identity.user (email);

CREATE TABLE identity.external_identity (
  id                  uuid PRIMARY KEY,
  user_id             uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  provider            text NOT NULL,
  provider_account_id text NOT NULL,
  linked_at           timestamptz NOT NULL,
  CONSTRAINT ext_provider_ck  CHECK (provider IN ('github','google')),
  CONSTRAINT ext_pai_len_ck   CHECK (char_length(provider_account_id) BETWEEN 1 AND 255));
CREATE UNIQUE INDEX ext_provider_uk
  ON identity.external_identity (provider, provider_account_id);
CREATE INDEX ext_user_ix ON identity.external_identity (user_id);

CREATE TABLE identity.dashboard_session (
  id             uuid PRIMARY KEY,
  user_id        uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  verifier       bytea NOT NULL,
  pepper_version smallint NOT NULL REFERENCES identity.credential_pepper(version),
  issued_at      timestamptz NOT NULL,
  expires_at     timestamptz NOT NULL,
  revoked_at     timestamptz,
  CONSTRAINT ds_verifier_len_ck CHECK (octet_length(verifier) = 32),
  CONSTRAINT ds_window_ck       CHECK (expires_at > issued_at),
  CONSTRAINT ds_cap_ck          CHECK (expires_at <= issued_at + INTERVAL '30 days'));
CREATE INDEX ds_user_ix   ON identity.dashboard_session (user_id);
CREATE INDEX ds_expiry_ix ON identity.dashboard_session (expires_at);

CREATE TABLE identity.email_challenge (
  id             uuid PRIMARY KEY,
  email          text NOT NULL,
  verifier       bytea NOT NULL,
  pepper_version smallint NOT NULL REFERENCES identity.credential_pepper(version),
  purpose        text NOT NULL,
  issued_at      timestamptz NOT NULL,
  expires_at     timestamptz NOT NULL,
  consumed_at    timestamptz,
  CONSTRAINT ec_purpose_ck      CHECK (purpose IN ('sign_in')),
  CONSTRAINT ec_verifier_len_ck CHECK (octet_length(verifier) = 32),
  CONSTRAINT ec_lower_ck        CHECK (email = lower(email)),
  CONSTRAINT ec_window_ck       CHECK (expires_at > issued_at));
CREATE INDEX ec_email_ix  ON identity.email_challenge (email, issued_at DESC);
CREATE INDEX ec_expiry_ix ON identity.email_challenge (expires_at);

CREATE TABLE identity.account_token (
  id             uuid PRIMARY KEY,
  user_id        uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  verifier       bytea NOT NULL,
  pepper_version smallint NOT NULL REFERENCES identity.credential_pepper(version),
  name           text NOT NULL,
  scopes         text[] NOT NULL,
  origin         text NOT NULL,
  issued_at      timestamptz NOT NULL,
  expires_at     timestamptz NOT NULL,
  revoked_at     timestamptz,
  CONSTRAINT at_verifier_len_ck CHECK (octet_length(verifier) = 32),
  CONSTRAINT at_origin_ck       CHECK (origin IN ('device_flow')),
  CONSTRAINT at_window_ck       CHECK (expires_at > issued_at),
  CONSTRAINT at_name_len_ck     CHECK (char_length(name) BETWEEN 1 AND 128),
  CONSTRAINT at_scopes_ck       CHECK (array_length(scopes,1) BETWEEN 1 AND 64));
CREATE INDEX at_user_ix ON identity.account_token (user_id) WHERE revoked_at IS NULL;

CREATE TABLE identity.device_authorization (
  id                   uuid PRIMARY KEY,
  device_verifier      bytea NOT NULL,
  user_code_hash       bytea NOT NULL,
  pepper_version       smallint NOT NULL REFERENCES identity.credential_pepper(version),
  status               text NOT NULL,
  requested_scopes     text[] NOT NULL,
  approved_by_user_id  uuid REFERENCES identity.user(id) ON DELETE RESTRICT,
  approved_at          timestamptz,
  consumed_at          timestamptz,
  account_token_id     uuid REFERENCES identity.account_token(id) ON DELETE RESTRICT,
  issued_at            timestamptz NOT NULL,
  expires_at           timestamptz NOT NULL,
  poll_interval_ms     integer NOT NULL,
  last_polled_at       timestamptz,
  CONSTRAINT dev_status_ck   CHECK (status IN ('pending','approved','denied','consumed','expired')),
  CONSTRAINT dev_approved_ck CHECK ((status IN ('approved','consumed')) = (approved_by_user_id IS NOT NULL)),
  CONSTRAINT dev_consumed_ck CHECK ((status = 'consumed') = (account_token_id IS NOT NULL)),
  CONSTRAINT dev_dv_len_ck   CHECK (octet_length(device_verifier) = 32),
  CONSTRAINT dev_uc_len_ck   CHECK (octet_length(user_code_hash) = 32),
  CONSTRAINT dev_window_ck   CHECK (expires_at > issued_at),
  CONSTRAINT dev_interval_ck CHECK (poll_interval_ms BETWEEN 1000 AND 60000));
CREATE UNIQUE INDEX dev_user_code_uk ON identity.device_authorization (user_code_hash);
CREATE INDEX dev_expiry_ix ON identity.device_authorization (expires_at);

-- A pepper may retire only when this view reports zero live references for it.
CREATE VIEW identity.credential_pepper_usage AS
SELECT pepper_version, count(*) AS live FROM (
            SELECT pepper_version FROM identity.account_token        WHERE revoked_at IS NULL
  UNION ALL SELECT pepper_version FROM identity.dashboard_session    WHERE revoked_at IS NULL
  UNION ALL SELECT pepper_version FROM identity.email_challenge      WHERE consumed_at IS NULL
  UNION ALL SELECT pepper_version FROM identity.device_authorization WHERE status IN ('pending','approved')
) s GROUP BY pepper_version;

-- Grants are an allowlist. There is no `REVOKE` denylist to keep in step with
-- dropped tables, which is exactly how the system this replaces acquired a
-- deploy-breaking revoke against a table that no longer existed.
GRANT USAGE ON SCHEMA identity TO aex_identity_api, aex_authz;

GRANT SELECT, INSERT, UPDATE ON
      identity.user,
      identity.external_identity,
      identity.dashboard_session,
      identity.email_challenge,
      identity.device_authorization,
      identity.account_token
  TO aex_identity_api;
GRANT DELETE ON identity.external_identity, identity.dashboard_session TO aex_identity_api;
GRANT SELECT ON identity.credential_pepper, identity.credential_pepper_usage TO aex_identity_api;

-- `central-authz` is read-only everywhere. Its readiness probe asserts that a
-- write fails, so a role misconfiguration is a start-up failure rather than a
-- runtime surprise.
GRANT SELECT ON
      identity.user,
      identity.dashboard_session,
      identity.account_token,
      identity.credential_pepper
  TO aex_authz;

GRANT SELECT ON identity.user TO aex_control_api, aex_control_worker;
