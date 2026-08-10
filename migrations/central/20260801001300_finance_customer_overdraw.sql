-- aex-migration: tx=yes destructive=no phase=expand
-- 20260801001300_finance_customer_overdraw.sql — lets the deduction that
-- exhausts an account actually be written, so the pause `20260801001200`
-- installed can fire.
--
-- # The defect this closes
--
-- Two rules claimed the same boundary and the wrong one arrived first.
-- `finance.apply_credit_exhaustion` pauses on `spendable <= 0`, and
-- `customer_balance_never_overdrawn` refused the very write that produces a
-- `spendable < 0`. So a real overspend did not pause the account: it aborted the
-- settlement posting, which left the usage unrecorded *and* the account running.
-- The `<` arm of the trigger was unreachable code, and `20260801001200`'s own
-- claim that "the account stops rather than the posting failing" was false the
-- day it was written, because the CHECK it deferred to got there first.
--
-- Deduction is asynchronous and batched by design. A balance that has gone past
-- zero is therefore not a corruption to be refused — it is the expected, and the
-- only, signal that the account must stop. Refusing to record it destroys the
-- signal along with the usage.
--
-- # Why the relaxation is exactly one account kind
--
-- The fence named two kinds and neither platform ledgers nor the journal itself,
-- which were never covered by it. Only `customer_available` moves:
--
--   * `customer_available` is what every existing money path posts to
--     (`top_up_settled`, `refund`, `dispute_opened`) and the only kind
--     `finance.apply_credit_exhaustion` reads — its first statement returns for
--     anything else. It is the account an overspend lands on, so it is the
--     account that has to be allowed to land.
--
--   * `customer_reserved` stays fenced. It is the escrow projection: its stored
--     balance is the negation of the sum of open holds, so a debit-positive
--     value does not mean "spent more than was funded", it means more was
--     settled or released out of escrow than was ever placed into it. That is a
--     conservation error, and it is the same law
--     `CHECK (settled_microusd + released_microusd <= reserved_microusd)` already
--     states per reservation, held here at the account level. No overspend needs
--     it to move, and no writer posts to it today.
--
-- # Why no floor replaces it
--
-- A bounded overdraw — "negative, but no further than X" — reintroduces the
-- identical defect at X instead of at zero: the settlement that crosses the
-- bound aborts and does not pause. Any ceiling is a fence, and a fence is what
-- is being removed. The overspend is bounded operationally, by the deduction
-- cadence and by the pause taking effect on the next admission, which is the
-- accepted trade. `balance_bound` still holds the column inside
-- ±1e17 microusd, so the row cannot run away.
--
-- # What still stops an exhausted account
--
-- Nothing on the request path reads the ledger and nothing here adds a read. The
-- chain is unchanged and now reachable end to end: the balance moves past zero →
-- `account_balance_credit_exhaustion` fires → `finance.billing_account` goes
-- `payment_hold` / `top_up_required` → the two state-change triggers bump the
-- revision and the account epoch and enqueue one `account.state.changed` per
-- live workspace in the same transaction → `central-control-worker` projects it
-- → `RegionalEdge::admit` refuses on the placement row it already fetches.
--
-- A never-funded account is now paused by its first posting rather than having
-- that posting refused. `20260801001200` argued the opposite from this CHECK
-- ("a posting against a zero credit-normal balance fails the overdraw CHECK");
-- that sentence is superseded here. The outcome it was protecting is preserved
-- and improved: the account still cannot spend on, and the usage is recorded
-- instead of discarded. The trigger stays `UPDATE`-only, so `ensure_account`
-- creating both balance rows at zero still pauses nobody.

ALTER TABLE finance.account_balance
  DROP CONSTRAINT customer_balance_never_overdrawn;

ALTER TABLE finance.account_balance
  ADD CONSTRAINT reserved_balance_never_overdrawn CHECK (
    kind <> 'customer_reserved' OR balance_microusd <= 0
  );
