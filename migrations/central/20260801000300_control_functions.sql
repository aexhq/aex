-- aex-migration: tx=yes destructive=no phase=baseline
-- 20260801000300_control_functions.sql — the epoch write path.
--
-- `control.authorization_epoch` receives no `INSERT`, `UPDATE` or `DELETE` from
-- any application role. The only way a row changes is through one of the five
-- per-kind `SECURITY DEFINER` wrappers below, each of which can only ever add
-- one. Monotonicity is therefore a database-level property: a decrement is not
-- merely forbidden, it is unrepresentable through the granted surface.
--
-- Which roles hold which wrapper, and the `REVOKE EXECUTE … FROM PUBLIC` that
-- makes the generic `control.bump_epoch(text, uuid)` unreachable, are declared
-- in `grants.toml`. It marks that generic form `never_granted = true`, so a
-- document that hands it to a role fails to load — a role holding one wrapper
-- cannot reach the generic form and bump another subject kind.

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
