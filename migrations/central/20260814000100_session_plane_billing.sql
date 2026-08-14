-- aex-migration: tx=yes destructive=yes phase=contract
-- The slim session-plane billing read model.
--
-- Stripe remains the card authority. This table stores only display metadata
-- from verified webhooks and an opaque provider reference needed for detach;
-- PAN, CVC, client secrets and hosted URLs cannot be represented here.

CREATE TABLE finance.payment_method (
  payment_method_id uuid PRIMARY KEY,
  org_id uuid NOT NULL REFERENCES control.organization (id) ON DELETE RESTRICT,
  provider_method_id text NOT NULL UNIQUE,
  brand text NOT NULL CHECK (brand IN (
    'visa', 'mastercard', 'amex', 'discover', 'diners', 'jcb', 'unionpay', 'unknown'
  )),
  last4 text NOT NULL CHECK (last4 ~ '^[0-9]{4}$'),
  expiry_month smallint NOT NULL CHECK (expiry_month BETWEEN 1 AND 12),
  expiry_year smallint NOT NULL CHECK (expiry_year BETWEEN 2020 AND 9999),
  state text NOT NULL CHECK (state IN ('attached', 'detached')),
  provider_created_at timestamptz NOT NULL,
  -- Projection order is provider event time, never Lambda arrival time. A stale
  -- attached/updated delivery therefore cannot resurrect a detached method.
  provider_updated_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX payment_method_account_created_idx
  ON finance.payment_method (org_id, created_at DESC, payment_method_id DESC)
  WHERE state = 'attached';

-- The public usage read exposes the charged quantity and optional model
-- attribution when an authority supplies them. Existing platform meters charge
-- their measured quantity, so that is the safe backfill/default.
ALTER TABLE finance.usage_inbox
  ADD COLUMN charged_quantity numeric(38, 0) NULL CHECK (charged_quantity >= 0),
  ADD COLUMN provider text NULL,
  ADD COLUMN model text NULL,
  ADD COLUMN session_id text NULL,
  ADD COLUMN token_class text NULL CHECK (token_class IN (
    'input', 'output', 'cache_read', 'cache_write', 'reasoning'
  ));

-- BYOK model observations share the one regional-to-central inbox, but do not
-- name a reservation or pricing context and never create a money transaction.
ALTER TABLE finance.usage_inbox
  ALTER COLUMN reservation_id DROP NOT NULL,
  ALTER COLUMN pricing_version DROP NOT NULL;

ALTER TABLE finance.usage_inbox
  ADD CONSTRAINT usage_inbox_model_shape_check CHECK (
    (category = 'model'
      AND reservation_id IS NULL
      AND pricing_version IS NULL
      AND basis = 'consumed'
      AND charged_quantity = 0
      AND ((state = 'rated' AND rated_microusd = 0)
        OR (state = 'quarantined' AND rated_microusd IS NULL))
      AND transaction_id IS NULL
      AND provider IS NOT NULL
      AND model IS NOT NULL
      AND session_id IS NOT NULL
      AND token_class IS NOT NULL)
    OR
    (category <> 'model'
      AND reservation_id IS NOT NULL
      AND pricing_version IS NOT NULL
      AND token_class IS NULL)
  );

ALTER TABLE finance.provider_effect
  DROP CONSTRAINT provider_effect_kind_check;
ALTER TABLE finance.provider_effect
  ADD CONSTRAINT provider_effect_kind_check CHECK (kind IN (
    'customer_create', 'checkout_session_create',
    'payment_method_session_create', 'payment_method_detach',
    'refund_create', 'tax_calculation_create', 'tax_transaction_create'
  ));
