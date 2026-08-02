-- aex-migration: tx=yes destructive=no phase=expand
-- 20260801000700_finance_account_state.sql — the two finance objects the
-- control plane calls, and the only two it is allowed to see.
--
-- Both existed as a fixture stub inside `aex-control-aurora`'s migration suite
-- and in no migration, so every control statement that joins finance state —
-- `RESOLVE_WORKSPACE_KEY`, the two workspace actor reads and `GET_ACCOUNT_STATE`
-- — parsed against a view production did not have. In production those reads
-- would have failed at `PREPARE`, not degraded.
--
-- `aex_control_api` and `aex_authz` hold `SELECT` on the view and nothing else
-- in `finance`; `aex_control_api` additionally holds `EXECUTE` on the function.
-- Both grants are declared in `grants.toml` beside the rest of the privilege
-- model.

-- The published projection of `finance.billing_account`.
--
-- A view rather than a grant on the table, for three reasons. It is the stable
-- name across a table the finance stream keeps changing; it exposes five
-- columns instead of a payment method id, a spend cap and a tax address; and it
-- translates the finance state machine into the three-value vocabulary the
-- authorization path understands (`aex_control_domain::AccountState`), so the
-- two never have to agree on a shared enum.
--
-- The translation is fail-closed. `active` is the only state that maps to
-- `active`; every other state — including one added later, which is what the
-- `ELSE` is for — maps to `paused_top_up_required`, the restrictive answer. An
-- organization with no row at all does not appear here, and the reading
-- statements `COALESCE` that to `unavailable`, which is `503`. Absence is never
-- `active`.
CREATE VIEW finance.account_state_v1 AS
SELECT b.org_id AS organization_id,
       CASE b.state WHEN 'active' THEN 'active'
                    ELSE 'paused_top_up_required' END AS status,
       b.state_reason AS reason,
       b.revision,
       b.updated_at AS changed_at
  FROM finance.billing_account b;

-- Establishes an organization's finance presence, idempotently.
--
-- `central-control-api` calls this in the **same transaction** as the
-- organization insert, so an organization can never exist without somewhere to
-- charge. It is `SECURITY DEFINER` precisely so that `aex_control_api` needs no
-- write privilege in `finance` to do it: the role holds `EXECUTE` on this one
-- function and `SELECT` on one view, and nothing else in the schema.
--
-- Idempotent by construction rather than by the caller checking first. Re-run,
-- concurrently or after a lost commit response, it converges and reports
-- whether *this* call created the billing account.
CREATE FUNCTION finance.ensure_account(p_organization_id uuid) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = finance, pg_temp AS $$
DECLARE created boolean;
BEGIN
  INSERT INTO finance.billing_account (org_id, state)
  VALUES (p_organization_id, 'active')
  ON CONFLICT (org_id) DO NOTHING;
  created := FOUND;

  -- The two customer accounts. `normal_side` is credit for both: a prepaid
  -- balance is a liability, so it is negative in the debit-positive chart, which
  -- is what `customer_balance_never_overdrawn` reads.
  INSERT INTO finance.account (account_id, org_id, kind, normal_side, currency)
  SELECT gen_random_uuid(), p_organization_id, wanted.kind, 'credit', 'USD'
    FROM (VALUES ('customer_available'::finance.account_kind),
                 ('customer_reserved'::finance.account_kind)) AS wanted (kind)
  ON CONFLICT (org_id, kind, currency) WHERE org_id IS NOT NULL DO NOTHING;

  INSERT INTO finance.account_balance (account_id, org_id, kind, currency)
  SELECT a.account_id, a.org_id, a.kind, a.currency
    FROM finance.account a
   WHERE a.org_id = p_organization_id
  ON CONFLICT (account_id) DO NOTHING;

  RETURN created;
END
$$;
