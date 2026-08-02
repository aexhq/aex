-- aex-migration: tx=yes destructive=no phase=baseline
-- 0003_control.sql — organizations, memberships, invitations, workspaces, API
-- key metadata, durable operations, replay records, audit, outbox and signing
-- keys.
--
-- Three invariants are database facts here rather than application care:
--
--   * an organization always has at least one active owner, enforced by a
--     `DEFERRABLE INITIALLY DEFERRED` constraint trigger so two transactions
--     racing to remove the last two owners cannot both win;
--   * a workspace's region and organization are immutable, enforced by a
--     `BEFORE UPDATE` trigger;
--   * `control.audit_event` and `control.authorization_epoch` receive no
--     `UPDATE` or `DELETE` grant anywhere, so neither can be quietly corrected.
--
-- An invitation carries **no secret**. There is no token column, because
-- acceptance is a verified-email match.

CREATE SCHEMA control;

CREATE TABLE control.credential_pepper (
  version     smallint PRIMARY KEY,
  purpose     text NOT NULL,
  state       text NOT NULL,
  secret_ref  text NOT NULL,
  created_at  timestamptz NOT NULL,
  retired_at  timestamptz,
  CONSTRAINT cpepper_purpose_ck CHECK (purpose IN ('api_key','cursor')),
  CONSTRAINT cpepper_state_ck   CHECK (state IN ('active','retiring','retired')),
  CONSTRAINT cpepper_retired_ck CHECK ((state = 'retired') = (retired_at IS NOT NULL)));
CREATE UNIQUE INDEX cpepper_active_uk
  ON control.credential_pepper (purpose) WHERE state = 'active';

CREATE TABLE control.organization (
  id                 uuid PRIMARY KEY,
  name               text NOT NULL,
  slug               text NOT NULL,
  status             text NOT NULL DEFAULT 'active',
  revision           bigint NOT NULL DEFAULT 1,
  created_at         timestamptz NOT NULL,
  updated_at         timestamptz NOT NULL,
  created_by_user_id uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  -- Organization deletion is absent from the accepted wire contract. A `CHECK`
  -- that admits one value makes the absence explicit rather than a forgotten
  -- state somebody later adds a transition to.
  CONSTRAINT org_status_ck CHECK (status IN ('active')),
  CONSTRAINT org_slug_ck   CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}[a-z0-9]$'),
  CONSTRAINT org_name_ck   CHECK (char_length(name) BETWEEN 1 AND 128));
CREATE UNIQUE INDEX org_slug_uk ON control.organization (slug);

CREATE TABLE control.membership (
  id              uuid PRIMARY KEY,
  organization_id uuid NOT NULL REFERENCES control.organization(id) ON DELETE RESTRICT,
  user_id         uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  role            text NOT NULL,
  status          text NOT NULL DEFAULT 'active',
  revision        bigint NOT NULL DEFAULT 1,
  created_at      timestamptz NOT NULL,
  updated_at      timestamptz NOT NULL,
  CONSTRAINT mem_role_ck   CHECK (role IN ('owner','admin','member')),
  CONSTRAINT mem_status_ck CHECK (status IN ('active','removed')));
CREATE UNIQUE INDEX mem_org_user_uk
  ON control.membership (organization_id, user_id) WHERE status = 'active';
CREATE INDEX mem_user_ix ON control.membership (user_id) WHERE status = 'active';

CREATE FUNCTION control.assert_org_has_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM control.membership
                 WHERE organization_id = COALESCE(NEW.organization_id, OLD.organization_id)
                   AND role = 'owner' AND status = 'active') THEN
    RAISE EXCEPTION 'organization % has no active owner',
      COALESCE(NEW.organization_id, OLD.organization_id)
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER membership_owner_required
  AFTER INSERT OR UPDATE OR DELETE ON control.membership
  DEFERRABLE INITIALLY DEFERRED
  FOR EACH ROW EXECUTE FUNCTION control.assert_org_has_owner();

CREATE TABLE control.invitation (
  id                  uuid PRIMARY KEY,
  organization_id     uuid NOT NULL REFERENCES control.organization(id) ON DELETE RESTRICT,
  email               text NOT NULL,
  role                text NOT NULL,
  status              text NOT NULL,
  invited_by_user_id  uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  accepted_user_id    uuid REFERENCES identity.user(id) ON DELETE RESTRICT,
  created_at          timestamptz NOT NULL,
  expires_at          timestamptz NOT NULL,
  resolved_at         timestamptz,
  -- `owner` is deliberately absent: ownership is transferred deliberately,
  -- never handed out by email.
  CONSTRAINT inv_role_ck     CHECK (role IN ('admin','member')),
  CONSTRAINT inv_status_ck   CHECK (status IN ('pending','accepted','revoked','expired')),
  CONSTRAINT inv_lower_ck    CHECK (email = lower(email)),
  CONSTRAINT inv_resolved_ck CHECK ((status = 'pending') = (resolved_at IS NULL)),
  CONSTRAINT inv_accepted_ck CHECK ((status = 'accepted') = (accepted_user_id IS NOT NULL)),
  CONSTRAINT inv_window_ck   CHECK (expires_at > created_at));
CREATE UNIQUE INDEX inv_pending_uk
  ON control.invitation (organization_id, email) WHERE status = 'pending';
CREATE INDEX inv_email_ix  ON control.invitation (email)      WHERE status = 'pending';
CREATE INDEX inv_expiry_ix ON control.invitation (expires_at) WHERE status = 'pending';

CREATE TABLE control.workspace (
  id                     uuid PRIMARY KEY,
  organization_id        uuid NOT NULL REFERENCES control.organization(id) ON DELETE RESTRICT,
  name                   text NOT NULL,
  slug                   text NOT NULL,
  region                 text NOT NULL,
  status                 text NOT NULL,
  provision_operation_id uuid NOT NULL,
  provision_fence        bigint NOT NULL,
  deletion_operation_id  uuid,
  deletion_fence         bigint,
  revision               bigint NOT NULL DEFAULT 1,
  created_at             timestamptz NOT NULL,
  updated_at             timestamptz NOT NULL,
  activated_at           timestamptz,
  deleted_at             timestamptz,
  created_by_user_id     uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  CONSTRAINT wsp_region_ck    CHECK (region IN ('us-east-1','us-east-2','us-west-2','ap-northeast-1','eu-west-1')),
  CONSTRAINT wsp_status_ck    CHECK (status IN ('provisioning','active','deleting','deleted')),
  CONSTRAINT wsp_slug_ck      CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}[a-z0-9]$'),
  CONSTRAINT wsp_name_ck      CHECK (char_length(name) BETWEEN 1 AND 128),
  CONSTRAINT wsp_activated_ck CHECK ((status IN ('active','deleting','deleted')) = (activated_at IS NOT NULL)),
  CONSTRAINT wsp_deletion_ck  CHECK ((status IN ('deleting','deleted')) = (deletion_operation_id IS NOT NULL)),
  CONSTRAINT wsp_deleted_ck   CHECK ((status = 'deleted') = (deleted_at IS NOT NULL)),
  CONSTRAINT wsp_fence_ck     CHECK (provision_fence >= 1));
CREATE UNIQUE INDEX wsp_org_slug_uk
  ON control.workspace (organization_id, slug) WHERE status <> 'deleted';
-- The composite unique index is what lets `control.api_key` carry a foreign key
-- over `(workspace_id, organization_id)`, so a key can never name a workspace
-- from another organization.
CREATE UNIQUE INDEX wsp_id_org_uk ON control.workspace (id, organization_id);
CREATE INDEX wsp_org_ix    ON control.workspace (organization_id, created_at, id)
  WHERE status <> 'provisioning';
CREATE INDEX wsp_region_ix ON control.workspace (region, status);

CREATE FUNCTION control.deny_workspace_placement_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.region <> OLD.region OR NEW.organization_id <> OLD.organization_id THEN
    RAISE EXCEPTION 'workspace placement is immutable'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER wsp_placement_immutable BEFORE UPDATE ON control.workspace
  FOR EACH ROW EXECUTE FUNCTION control.deny_workspace_placement_change();

CREATE TABLE control.api_key (
  id                 uuid PRIMARY KEY,
  workspace_id       uuid NOT NULL,
  organization_id    uuid NOT NULL,
  name               text NOT NULL,
  scopes             text[] NOT NULL,
  region             text NOT NULL,
  verifier           bytea NOT NULL,
  pepper_version     smallint NOT NULL REFERENCES control.credential_pepper(version),
  created_at         timestamptz NOT NULL,
  revoked_at         timestamptz,
  revision           bigint NOT NULL DEFAULT 1,
  created_by_user_id uuid NOT NULL REFERENCES identity.user(id) ON DELETE RESTRICT,
  FOREIGN KEY (workspace_id, organization_id)
    REFERENCES control.workspace(id, organization_id) ON DELETE RESTRICT,
  CONSTRAINT key_verifier_len_ck CHECK (octet_length(verifier) = 32),
  CONSTRAINT key_name_ck         CHECK (char_length(name) BETWEEN 1 AND 128),
  CONSTRAINT key_scopes_ck       CHECK (array_length(scopes,1) BETWEEN 1 AND 64),
  CONSTRAINT key_region_ck       CHECK (region IN ('us-east-1','us-east-2','us-west-2','ap-northeast-1','eu-west-1')));
CREATE INDEX key_workspace_ix ON control.api_key (workspace_id, created_at, id);
CREATE INDEX key_pepper_ix    ON control.api_key (pepper_version) WHERE revoked_at IS NULL;

CREATE TABLE control.authorization_epoch (
  subject_kind text NOT NULL,
  subject_id   uuid NOT NULL,
  epoch        bigint NOT NULL,
  updated_at   timestamptz NOT NULL,
  PRIMARY KEY (subject_kind, subject_id),
  CONSTRAINT epoch_kind_ck     CHECK (subject_kind IN ('user','membership','workspace','key','account')),
  CONSTRAINT epoch_positive_ck CHECK (epoch >= 1));

CREATE TABLE control.durable_operation (
  id                uuid PRIMARY KEY,
  kind              text NOT NULL,
  visibility        text NOT NULL,
  organization_id   uuid NOT NULL REFERENCES control.organization(id) ON DELETE RESTRICT,
  workspace_id      uuid REFERENCES control.workspace(id) ON DELETE RESTRICT,
  principal_kind    text NOT NULL,
  principal_id      uuid NOT NULL,
  scopes            text[] NOT NULL,
  status            text NOT NULL,
  intent_hash       bytea NOT NULL,
  fence             bigint NOT NULL,
  attempt           integer NOT NULL DEFAULT 0,
  lease_owner       text,
  lease_expires_at  timestamptz,
  progress          jsonb,
  result            jsonb,
  error             jsonb,
  created_at        timestamptz NOT NULL,
  started_at        timestamptz,
  updated_at        timestamptz NOT NULL,
  committed_at      timestamptz,
  terminal_at       timestamptz,
  due_at            timestamptz,
  CONSTRAINT op_kind_ck     CHECK (kind IN ('workspace_provision','workspace_delete')),
  CONSTRAINT op_vis_ck      CHECK (visibility IN ('public','internal')),
  CONSTRAINT op_status_ck   CHECK (status IN ('queued','running','succeeded','failed','cancelled')),
  CONSTRAINT op_principal_ck CHECK (principal_kind IN ('account_actor','workspace_key','system')),
  CONSTRAINT op_intent_ck   CHECK (octet_length(intent_hash) = 32),
  CONSTRAINT op_terminal_ck CHECK ((status IN ('succeeded','failed','cancelled')) = (terminal_at IS NOT NULL)),
  CONSTRAINT op_result_ck   CHECK (NOT (status = 'succeeded' AND result IS NULL)),
  CONSTRAINT op_error_ck    CHECK (NOT (status = 'failed' AND error IS NULL)),
  CONSTRAINT op_lease_ck    CHECK ((lease_owner IS NULL) = (lease_expires_at IS NULL)),
  CONSTRAINT op_fence_ck    CHECK (fence >= 1),
  CONSTRAINT op_attempt_ck  CHECK (attempt BETWEEN 0 AND 100));
CREATE INDEX op_due_ix ON control.durable_operation (due_at)
  WHERE status IN ('queued','running');
CREATE INDEX op_org_ix ON control.durable_operation (organization_id, created_at DESC, id)
  WHERE visibility = 'public';

CREATE TABLE control.idempotency_record (
  id              uuid PRIMARY KEY,
  key_kind        text NOT NULL,
  key_value       text NOT NULL,
  principal_kind  text NOT NULL,
  principal_id    uuid NOT NULL,
  scope_kind      text NOT NULL,
  scope_id        uuid NOT NULL,
  method          text NOT NULL,
  route           text NOT NULL,
  intent_hash     bytea NOT NULL,
  state           text NOT NULL,
  response_status smallint,
  response_body   jsonb,
  operation_id    uuid REFERENCES control.durable_operation(id) ON DELETE RESTRICT,
  created_at      timestamptz NOT NULL,
  completed_at    timestamptz,
  expires_at      timestamptz NOT NULL,
  CONSTRAINT idem_kind_ck      CHECK (key_kind IN ('idempotency_key','operation_id')),
  CONSTRAINT idem_scope_ck     CHECK (scope_kind IN ('organization','workspace')),
  CONSTRAINT idem_state_ck     CHECK (state IN ('in_flight','completed')),
  CONSTRAINT idem_completed_ck CHECK ((state = 'completed') = (completed_at IS NOT NULL)),
  CONSTRAINT idem_response_ck  CHECK ((state = 'completed') = (response_status IS NOT NULL)),
  CONSTRAINT idem_intent_ck    CHECK (octet_length(intent_hash) = 32),
  CONSTRAINT idem_key_len_ck   CHECK (char_length(key_value) BETWEEN 1 AND 255));
CREATE UNIQUE INDEX idem_identity_uk ON control.idempotency_record
  (key_kind, key_value, principal_kind, principal_id, scope_kind, scope_id, method, route);
-- The expiry index is what makes the 24-hour sweep a range scan rather than a
-- full table scan; the system this replaces had neither the column nor a reader.
CREATE INDEX idem_expiry_ix ON control.idempotency_record (expires_at);

CREATE TABLE control.audit_event (
  id              uuid PRIMARY KEY,
  organization_id uuid,
  workspace_id    uuid,
  actor_kind      text NOT NULL,
  actor_id        uuid,
  action          text NOT NULL,
  resource_kind   text NOT NULL,
  resource_id     uuid,
  outcome         text NOT NULL,
  request_id      text NOT NULL,
  operation_id    uuid,
  detail          jsonb NOT NULL DEFAULT '{}'::jsonb,
  occurred_at     timestamptz NOT NULL,
  CONSTRAINT audit_actor_ck   CHECK (actor_kind IN ('user','workspace_key','system')),
  CONSTRAINT audit_outcome_ck CHECK (outcome IN ('allowed','denied')));
CREATE INDEX audit_org_ix ON control.audit_event (organization_id, occurred_at DESC, id);
CREATE INDEX audit_resource_ix ON control.audit_event (resource_kind, resource_id, occurred_at DESC);

CREATE TABLE control.outbox_message (
  id            uuid PRIMARY KEY,
  topic         text NOT NULL,
  dedupe_key    text NOT NULL,
  group_key     text NOT NULL,
  payload       jsonb NOT NULL,
  attempts      integer NOT NULL DEFAULT 0,
  available_at  timestamptz NOT NULL,
  claimed_by    text,
  claimed_until timestamptz,
  dispatched_at timestamptz,
  last_error    text,
  created_at    timestamptz NOT NULL,
  CONSTRAINT outbox_topic_ck CHECK (topic IN (
    'workspace.provision.requested',
    'workspace.delete.requested',
    'invitation.email.requested',
    'authorization.epoch.changed',
    'authorization.signing_key.published')),
  CONSTRAINT outbox_claim_ck    CHECK ((claimed_by IS NULL) = (claimed_until IS NULL)),
  CONSTRAINT outbox_attempts_ck CHECK (attempts BETWEEN 0 AND 100));
CREATE UNIQUE INDEX outbox_dedupe_uk ON control.outbox_message (topic, dedupe_key);
CREATE INDEX outbox_pending_ix ON control.outbox_message (available_at, id)
  WHERE dispatched_at IS NULL;

CREATE TABLE control.signing_key (
  kid          uuid PRIMARY KEY,
  alg          text NOT NULL,
  public_key   bytea NOT NULL,
  secret_ref   text NOT NULL,
  state        text NOT NULL,
  created_at   timestamptz NOT NULL,
  activates_at timestamptz NOT NULL,
  retires_at   timestamptz NOT NULL,
  retired_at   timestamptz,
  CONSTRAINT sk_alg_ck        CHECK (alg = 'ed25519'),
  CONSTRAINT sk_public_len_ck CHECK (octet_length(public_key) = 32),
  CONSTRAINT sk_state_ck      CHECK (state IN ('pending','active','retiring','retired')),
  CONSTRAINT sk_window_ck     CHECK (retires_at > activates_at));
CREATE UNIQUE INDEX sk_active_uk ON control.signing_key ((true)) WHERE state = 'active';
