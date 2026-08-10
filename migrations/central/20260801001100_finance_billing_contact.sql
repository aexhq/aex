-- aex-migration: tx=yes destructive=no phase=expand
-- 20260801001100_finance_billing_contact.sql — the address `EnsureCustomer`
-- creates a provider customer record against.
--
-- `finance.billing_account.provider_customer_id` had no writer anywhere, so
-- `prepare_effect` refused every hosted command with "the organization has no
-- provider customer record yet" and both `billing_top_up_checkout_create` and
-- `billing_portal_session_create` answered `400 invalid_request` for every input.
-- No money could enter. Creating the customer needs an address, and finance
-- holds none: `finance.billing_account` carries a provider customer id, a
-- payment method id and a tax address, and deliberately no email, because a
-- second copy of a person's address is a second thing to keep correct.
--
-- So the address is read from identity, through a `SECURITY DEFINER` function,
-- which is `finance.ensure_account` written the other way round: the calling
-- role holds no privilege in the schema it reaches, and the reach is one column
-- of one row rather than a grant on `identity.user`. `aex_finance_api` gains
-- `EXECUTE` on this and nothing else in `identity`; the grant is declared in
-- `grants.toml` beside the rest of the privilege model.
--
-- The contact is the organization's creator. `control.organization.created_by_user_id`
-- is `NOT NULL` and `ON DELETE RESTRICT`, so the row always resolves; membership
-- roles change and the creator does not, and a provider customer record is
-- created exactly once per organization. A customer who wants a different
-- address changes it in the hosted portal, after which the address is the
-- provider's fact and not ours.
--
-- `STABLE`, not `VOLATILE`: it reads and never writes, so the planner may call
-- it once per statement.
CREATE FUNCTION finance.billing_contact_email(p_organization_id uuid) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER
SET search_path = finance, control, identity, pg_temp AS $$
  SELECT u.email
    FROM control.organization o
    JOIN identity.user u ON u.id = o.created_by_user_id
   WHERE o.id = p_organization_id;
$$;
