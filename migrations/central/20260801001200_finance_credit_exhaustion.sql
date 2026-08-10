-- aex-migration: tx=yes destructive=no phase=expand
-- 20260801001200_finance_credit_exhaustion.sql — the writer
-- `finance.billing_account.state` never had.
--
-- Everything downstream of this column was already built and correct: the two
-- state-change triggers bump the revision and the account epoch and enqueue one
-- deduplicated `account.state.changed` per live workspace in the *same*
-- transaction; `central-control-worker` projects it onto the regional placement
-- row; `RegionalEdge::admit` stage 8 refuses every non-pause-exempt route with
-- `402`; and `revalidate` terminates open leases within one heartbeat. None of it
-- could ever fire, because the column was inserted `'active'` and updated by
-- nothing. The pause gate, its `402` arm and both non-active arms of the control
-- plane's account projection were unreachable code.
--
-- # Why a trigger rather than a caller
--
-- The balance moves from four different roles — ingest applies a top-up, refund
-- and dispute claw-back; settlement will post usage; reconcile repairs. A rule
-- each of them has to remember to apply is a rule that will be forgotten by
-- whichever one is written last, and the failure mode is silent: the customer
-- keeps spending. Attached to the row, the rule cannot be skipped, and it holds
-- for a writer that does not exist yet.
--
-- # The floor, and why it is exactly zero
--
-- `customer_available` is credit-normal, so the spendable amount is the negation
-- of the stored balance, and `customer_balance_never_overdrawn` already makes a
-- balance that would go debit-positive a transaction failure. The floor is
-- therefore `spendable <= 0` and needs no constant: it is the same boundary the
-- CHECK enforces, evaluated one step earlier so the account stops rather than the
-- posting failing.
--
-- Stopping *at* zero is not the same as stopping *exactly* at zero, and the
-- accepted design is the second. Nothing on the request path reads the ledger;
-- the mark is written here and read from the placement row the admission snapshot
-- already fetches, so the overshoot is the propagation delay — p99 ≤ 30 s with a
-- 120 s ceiling, alarmed rather than fail-closed at that layer.
--
-- # `UPDATE` only, deliberately
--
-- `finance.ensure_account` inserts both balance rows at zero, so an `INSERT` arm
-- would pause every organization the moment it is created. Whether an account
-- that has never been funded may run at all is a product question with a real
-- instrument behind it — the one-time launch credit — and inventing an answer
-- here would pause every new customer before anyone chose to. This trigger
-- detects a *transition* of the balance, which is what "stop at zero" means; a
-- never-funded account cannot spend in any case, because a posting against a zero
-- credit-normal balance fails the overdraw CHECK.
--
-- # What it must never do
--
-- It flips `active -> payment_hold` on exhaustion and `payment_hold -> active` on
-- funding, and it touches nothing else. `dispute_hold` and `closed` are a
-- chargeback and a terminated account: they are not shortages, a top-up does not
-- clear them, and an automatic resume out of either would hand service back on
-- money that is being reclaimed. The guard is the `WHERE`, not a convention.
--
-- `state_reason` is the customer-facing **remedy**, spelled in the published
-- `AccountPauseReason` vocabulary that `aex_control_domain::AccountPauseCause`
-- parses. It is a different fact from `state`, which is the internal hold: two
-- holds can share one remedy, which is why `finance.account_state_v1` collapses
-- the four holds to one status and republishes the reason unchanged.
CREATE FUNCTION finance.apply_credit_exhaustion() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = finance, pg_temp AS $$
BEGIN
  IF NEW.kind <> 'customer_available' OR NEW.org_id IS NULL THEN
    RETURN NULL;
  END IF;

  IF -NEW.balance_microusd <= 0 THEN
    UPDATE finance.billing_account
       SET state = 'payment_hold', state_reason = 'top_up_required'
     WHERE org_id = NEW.org_id AND state = 'active';
  ELSE
    UPDATE finance.billing_account
       SET state = 'active', state_reason = NULL
     WHERE org_id = NEW.org_id
       AND state = 'payment_hold' AND state_reason = 'top_up_required';
  END IF;

  RETURN NULL;
END
$$;

-- `SECURITY DEFINER` because the rule belongs to the row and not to the role that
-- moved it: `aex_finance_settlement` and `aex_finance_reconcile` hold no privilege
-- on `finance.billing_account` at all, and giving each of them one would make the
-- pause a privilege four roles share rather than a property of the balance.
CREATE TRIGGER account_balance_credit_exhaustion
AFTER UPDATE OF balance_microusd ON finance.account_balance
FOR EACH ROW EXECUTE FUNCTION finance.apply_credit_exhaustion();
