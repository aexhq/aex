-- 0004_control_functions.sql — the epoch write path and every control grant.
--
-- `control.authorization_epoch` receives no `UPDATE` or `DELETE` grant from any
-- application role. The only way a row changes is through one of the five
-- per-kind `SECURITY DEFINER` wrappers, each of which can only ever add one.
-- Monotonicity is therefore a database-level property: a decrement is not
-- merely forbidden, it is unrepresentable through the granted surface.
--
-- `control.bump_epoch(text, uuid)` itself is revoked from `PUBLIC`, so a role
-- that holds one wrapper cannot reach the generic form and bump another kind.

CREATE FUNCTION control.bump_epoch(p_kind text, p_id uuid) RETURNS bigint LANGUAGE sql AS $$
  INSERT INTO control.authorization_epoch (subject_kind, subject_id, epoch, updated_at)
  VALUES (p_kind, p_id, 1, now())
  ON CONFLICT (subject_kind, subject_id)
  DO UPDATE SET epoch = control.authorization_epoch.epoch + 1, updated_at = now()
  RETURNING epoch;
$$;

CREATE FUNCTION control.bump_user_epoch(p uuid) RETURNS bigint LANGUAGE sql SECURITY DEFINER
  SET search_path = control, pg_temp AS $$ SELECT control.bump_epoch('user', p) $$;
CREATE FUNCTION control.bump_membership_epoch(p uuid) RETURNS bigint LANGUAGE sql SECURITY DEFINER
  SET search_path = control, pg_temp AS $$ SELECT control.bump_epoch('membership', p) $$;
CREATE FUNCTION control.bump_workspace_epoch(p uuid) RETURNS bigint LANGUAGE sql SECURITY DEFINER
  SET search_path = control, pg_temp AS $$ SELECT control.bump_epoch('workspace', p) $$;
CREATE FUNCTION control.bump_key_epoch(p uuid) RETURNS bigint LANGUAGE sql SECURITY DEFINER
  SET search_path = control, pg_temp AS $$ SELECT control.bump_epoch('key', p) $$;
CREATE FUNCTION control.bump_account_epoch(p uuid) RETURNS bigint LANGUAGE sql SECURITY DEFINER
  SET search_path = control, pg_temp AS $$ SELECT control.bump_epoch('account', p) $$;

REVOKE EXECUTE ON FUNCTION control.bump_epoch(text, uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION control.bump_user_epoch(uuid)       FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION control.bump_membership_epoch(uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION control.bump_workspace_epoch(uuid)  FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION control.bump_key_epoch(uuid)        FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION control.bump_account_epoch(uuid)    FROM PUBLIC;

GRANT USAGE ON SCHEMA control
  TO aex_control_api, aex_control_worker, aex_authz, aex_identity_api;

-- central-control-api ---------------------------------------------------------
GRANT SELECT, INSERT, UPDATE ON
      control.organization,
      control.membership,
      control.invitation,
      control.workspace,
      control.api_key,
      control.durable_operation,
      control.idempotency_record
  TO aex_control_api;
-- Audit and outbox are append-only for every application role.
GRANT SELECT, INSERT ON control.audit_event, control.outbox_message TO aex_control_api;
GRANT SELECT ON control.authorization_epoch, control.credential_pepper TO aex_control_api;
GRANT EXECUTE ON FUNCTION
      control.bump_key_epoch(uuid),
      control.bump_membership_epoch(uuid),
      control.bump_workspace_epoch(uuid)
  TO aex_control_api;

-- central-control-worker ------------------------------------------------------
GRANT SELECT, UPDATE ON
      control.workspace,
      control.durable_operation,
      control.invitation,
      control.outbox_message
  TO aex_control_worker;
-- The worker is the only role that reclaims expired rows, and only from the two
-- tables whose retention is a policy rather than a fact.
GRANT DELETE ON control.outbox_message, control.idempotency_record TO aex_control_worker;
GRANT SELECT, INSERT ON control.audit_event TO aex_control_worker;
GRANT SELECT, INSERT, UPDATE ON control.signing_key TO aex_control_worker;
GRANT SELECT ON
      control.authorization_epoch,
      control.credential_pepper,
      control.api_key,
      control.organization,
      control.membership,
      control.idempotency_record
  TO aex_control_worker;
GRANT EXECUTE ON FUNCTION
      control.bump_key_epoch(uuid),
      control.bump_workspace_epoch(uuid),
      control.bump_membership_epoch(uuid)
  TO aex_control_worker;

-- central-authz ---------------------------------------------------------------
-- Read-only, everywhere. A write probe failing is a start-up assertion.
GRANT SELECT ON
      control.organization,
      control.membership,
      control.workspace,
      control.api_key,
      control.authorization_epoch,
      control.signing_key,
      control.credential_pepper
  TO aex_authz;

-- central-identity-api --------------------------------------------------------
-- Exactly one control privilege: advancing the `user` epoch when a person is
-- disabled or loses their last live token.
GRANT EXECUTE ON FUNCTION control.bump_user_epoch(uuid) TO aex_identity_api;

-- Never granted to any application role, anywhere:
--   * UPDATE or DELETE on control.authorization_epoch
--   * UPDATE or DELETE on control.audit_event
--   * EXECUTE on control.bump_epoch(text, uuid)
--   * any DDL, any CREATE on a schema, any ownership
-- `aex_control_*` hold no privilege in `finance`; `aex_finance_*` hold none in
-- `identity` or `control` beyond the two objects granted by
-- `0006_cross_schema_grants.sql`.
