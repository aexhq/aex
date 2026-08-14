-- aex-migration: tx=yes destructive=no phase=expand
-- The one-person launch account and its UUIDv7-funded ledger bootstrap.
--
-- `control.organization` remains the internal relational and money key, but a
-- public user can own exactly one such row and exactly one fixed workspace.
-- This table makes that launch invariant a database fact instead of relying on
-- bootstrap to select the first row from the former multi-organization model.

ALTER TABLE control.membership
  ADD CONSTRAINT mem_id_org_user_uk UNIQUE (id, organization_id, user_id);

CREATE TABLE control.personal_account (
  account_id uuid PRIMARY KEY REFERENCES control.organization (id) ON DELETE RESTRICT,
  user_id uuid NOT NULL UNIQUE REFERENCES identity.user (id) ON DELETE RESTRICT,
  membership_id uuid NOT NULL UNIQUE,
  workspace_id uuid NOT NULL UNIQUE,
  created_at timestamptz NOT NULL,
  FOREIGN KEY (membership_id, account_id, user_id)
    REFERENCES control.membership (id, organization_id, user_id) ON DELETE RESTRICT,
  FOREIGN KEY (workspace_id, account_id)
    REFERENCES control.workspace (id, organization_id) ON DELETE RESTRICT
);

-- Establishes the two prepaid customer accounts with caller-minted UUIDv7
-- identities. The previous generic helper generated UUIDv4 values inside
-- PostgreSQL, which made first-login identity ordering unverifiable.
CREATE FUNCTION finance.ensure_personal_ledger_accounts(
  p_account_id uuid,
  p_available_account_id uuid,
  p_reserved_account_id uuid
) RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = finance, pg_temp
AS $$
BEGIN
  INSERT INTO finance.billing_account (org_id, state)
  VALUES (p_account_id, 'active');

  INSERT INTO finance.account (account_id, org_id, kind, normal_side, currency)
  VALUES
    (p_available_account_id, p_account_id, 'customer_available', 'credit', 'USD'),
    (p_reserved_account_id, p_account_id, 'customer_reserved', 'credit', 'USD');

  INSERT INTO finance.account_balance (account_id, org_id, kind, currency)
  VALUES
    (p_available_account_id, p_account_id, 'customer_available', 'USD'),
    (p_reserved_account_id, p_account_id, 'customer_reserved', 'USD');
END
$$;

-- Control needs the two opaque ledger identities to reconcile a completed
-- first login, but it must not receive a broad read grant on the ledger. This
-- projection exposes only that fixed relationship and no balances or history.
CREATE VIEW finance.personal_ledger_accounts AS
SELECT available.org_id AS account_id,
       available.account_id AS available_account_id,
       reserved.account_id AS reserved_account_id
  FROM finance.account available
  JOIN finance.account reserved
    ON reserved.org_id = available.org_id
   AND reserved.currency = available.currency
   AND reserved.kind = 'customer_reserved'
 WHERE available.org_id IS NOT NULL
   AND available.currency = 'USD'
   AND available.kind = 'customer_available';
