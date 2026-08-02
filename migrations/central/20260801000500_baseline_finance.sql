-- aex-migration: tx=yes destructive=no phase=baseline

CREATE TYPE finance.currency AS ENUM ('USD');
CREATE TYPE finance.account_side AS ENUM ('debit', 'credit');
CREATE TYPE finance.account_kind AS ENUM (
  'provider_clearing', 'processor_fee_expense', 'customer_available',
  'customer_reserved', 'usage_revenue', 'tax_payable', 'refund_clearing',
  'dispute_holding', 'dispute_loss_expense', 'goodwill_expense'
);
CREATE TYPE finance.transaction_kind AS ENUM (
  'top_up_settled', 'processor_fee', 'usage_reserve', 'usage_settlement',
  'reservation_release', 'refund', 'dispute_opened', 'dispute_closed',
  'goodwill_credit', 'tax_adjustment', 'reversal'
);

CREATE TABLE finance.account (
  account_id uuid PRIMARY KEY,
  org_id uuid NULL REFERENCES control.organization (org_id) ON DELETE RESTRICT,
  kind finance.account_kind NOT NULL,
  normal_side finance.account_side NOT NULL,
  currency finance.currency NOT NULL DEFAULT 'USD',
  created_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT account_scope_chk CHECK (
    (kind IN ('customer_available', 'customer_reserved')) = (org_id IS NOT NULL)
  )
);
CREATE UNIQUE INDEX account_org_kind_uniq
  ON finance.account (org_id, kind, currency) WHERE org_id IS NOT NULL;
CREATE UNIQUE INDEX account_platform_kind_uniq
  ON finance.account (kind, currency) WHERE org_id IS NULL;

CREATE TABLE finance.journal_transaction (
  transaction_id uuid PRIMARY KEY,
  org_id uuid NULL,
  kind finance.transaction_kind NOT NULL,
  business_key text NOT NULL,
  intent_hash bytea NOT NULL,
  reverses_transaction_id uuid NULL
    REFERENCES finance.journal_transaction (transaction_id),
  posting_count smallint NOT NULL CHECK (posting_count >= 2),
  occurred_at timestamptz NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT jt_intent_hash_len CHECK (octet_length(intent_hash) = 32),
  CONSTRAINT jt_not_future CHECK (recorded_at <= now() + interval '1 minute'),
  CONSTRAINT jt_business_key_len CHECK (length(business_key) BETWEEN 8 AND 512)
);
CREATE UNIQUE INDEX journal_transaction_business_key_uniq
  ON finance.journal_transaction (business_key);
CREATE UNIQUE INDEX journal_transaction_reversal_uniq
  ON finance.journal_transaction (reverses_transaction_id)
  WHERE reverses_transaction_id IS NOT NULL;
CREATE INDEX journal_transaction_org_time_idx
  ON finance.journal_transaction (org_id, occurred_at DESC, transaction_id DESC);

CREATE TABLE finance.journal_posting (
  transaction_id uuid NOT NULL
    REFERENCES finance.journal_transaction (transaction_id),
  posting_seq smallint NOT NULL,
  account_id uuid NOT NULL REFERENCES finance.account (account_id),
  currency finance.currency NOT NULL,
  amount_microusd bigint NOT NULL,
  PRIMARY KEY (transaction_id, posting_seq),
  CONSTRAINT posting_nonzero CHECK (amount_microusd <> 0),
  CONSTRAINT posting_bound CHECK (
    amount_microusd BETWEEN -1000000000000000 AND 1000000000000000
  )
);
CREATE INDEX journal_posting_account_idx
  ON finance.journal_posting (account_id) INCLUDE (amount_microusd);

CREATE FUNCTION finance.assert_transaction_balanced()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  target_id uuid;
  imbalance bigint;
  observed integer;
  declared integer;
BEGIN
  target_id := NEW.transaction_id;
  SELECT coalesce(sum(amount_microusd), 0), count(*)
    INTO imbalance, observed
    FROM finance.journal_posting
    WHERE transaction_id = target_id;
  SELECT posting_count INTO declared
    FROM finance.journal_transaction
    WHERE transaction_id = target_id;
  IF declared IS NULL OR observed <> declared THEN
    RAISE EXCEPTION 'finance: txn % has % postings, declared %',
      target_id, observed, declared USING ERRCODE = '23514';
  END IF;
  IF imbalance <> 0 THEN
    RAISE EXCEPTION 'finance: txn % unbalanced by % microusd',
      target_id, imbalance USING ERRCODE = '23514';
  END IF;
  RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER journal_posting_balanced
  AFTER INSERT ON finance.journal_posting
  DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
  EXECUTE FUNCTION finance.assert_transaction_balanced();
CREATE CONSTRAINT TRIGGER journal_transaction_balanced
  AFTER INSERT ON finance.journal_transaction
  DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
  EXECUTE FUNCTION finance.assert_transaction_balanced();

CREATE FUNCTION finance.deny_history_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'finance: % on % is forbidden; correct with a reversal',
    TG_OP, TG_TABLE_NAME USING ERRCODE = '42501';
END
$$;
CREATE TRIGGER journal_transaction_append_only
  BEFORE UPDATE OR DELETE ON finance.journal_transaction
  FOR EACH ROW EXECUTE FUNCTION finance.deny_history_mutation();
CREATE TRIGGER journal_posting_append_only
  BEFORE UPDATE OR DELETE ON finance.journal_posting
  FOR EACH ROW EXECUTE FUNCTION finance.deny_history_mutation();

CREATE TABLE finance.account_balance (
  account_id uuid PRIMARY KEY REFERENCES finance.account (account_id),
  org_id uuid NULL,
  kind finance.account_kind NOT NULL,
  currency finance.currency NOT NULL,
  balance_microusd bigint NOT NULL DEFAULT 0,
  posting_count bigint NOT NULL DEFAULT 0,
  last_transaction_id uuid NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT balance_bound CHECK (
    balance_microusd BETWEEN -100000000000000000 AND 100000000000000000
  ),
  CONSTRAINT customer_balance_never_overdrawn CHECK (
    kind NOT IN ('customer_available', 'customer_reserved')
    OR balance_microusd <= 0
  )
);

CREATE TABLE finance.pricing_context (
  pricing_version text PRIMARY KEY,
  content_sha256 bytea NOT NULL CHECK (octet_length(content_sha256) = 32),
  signature bytea NOT NULL,
  rate_book jsonb NOT NULL,
  rounding_rule text NOT NULL CHECK (rounding_rule IN ('half_even', 'ceil_total')),
  currency finance.currency NOT NULL DEFAULT 'USD',
  billing_active boolean NOT NULL DEFAULT false,
  effective_from timestamptz NOT NULL,
  effective_to timestamptz NULL,
  CHECK (effective_to IS NULL OR effective_to > effective_from),
  CHECK (rate_book ->> 'pricingVersion' = pricing_version),
  CHECK (rate_book ->> 'roundingRule' = rounding_rule)
);

CREATE TABLE finance.reservation (
  reservation_id uuid PRIMARY KEY,
  org_id uuid NOT NULL REFERENCES control.organization (org_id),
  workspace_id uuid NOT NULL,
  region text NOT NULL,
  scope_kind text NOT NULL CHECK (scope_kind IN ('session', 'run', 'operation')),
  scope_id text NOT NULL,
  state text NOT NULL CHECK (state IN ('open', 'closing', 'settled', 'released', 'voided')),
  reserved_microusd bigint NOT NULL CHECK (reserved_microusd >= 0),
  settled_microusd bigint NOT NULL DEFAULT 0 CHECK (settled_microusd >= 0),
  released_microusd bigint NOT NULL DEFAULT 0 CHECK (released_microusd >= 0),
  pricing_version text NOT NULL REFERENCES finance.pricing_context (pricing_version),
  revision bigint NOT NULL DEFAULT 0,
  CHECK (settled_microusd + released_microusd <= reserved_microusd),
  UNIQUE (org_id, scope_kind, scope_id)
);

CREATE TABLE finance.reservation_closure (
  reservation_id uuid PRIMARY KEY REFERENCES finance.reservation (reservation_id),
  declared jsonb NOT NULL,
  satisfied jsonb NOT NULL DEFAULT '{}',
  state text NOT NULL CHECK (state IN ('pending', 'satisfied', 'released'))
);

CREATE TABLE finance.usage_inbox (
  region text NOT NULL,
  category text NOT NULL,
  fact_id text NOT NULL,
  intent_hash bytea NOT NULL CHECK (octet_length(intent_hash) = 32),
  org_id uuid NOT NULL,
  workspace_id uuid NOT NULL,
  reservation_id uuid NOT NULL REFERENCES finance.reservation (reservation_id),
  meter text NOT NULL,
  basis text NOT NULL CHECK (basis IN ('consumed', 'reserved')),
  quantity numeric(38, 0) NOT NULL CHECK (quantity >= 0),
  interval_start timestamptz NOT NULL,
  interval_end timestamptz NOT NULL CHECK (interval_end >= interval_start),
  pricing_version text NOT NULL REFERENCES finance.pricing_context (pricing_version),
  corrects_fact_id text NULL,
  accepted_sequence bigint NOT NULL,
  state text NOT NULL CHECK (state IN ('pending', 'rated', 'quarantined')),
  rated_microusd bigint NULL,
  transaction_id uuid NULL REFERENCES finance.journal_transaction (transaction_id),
  PRIMARY KEY (region, category, fact_id)
);
CREATE INDEX usage_inbox_pending_idx
  ON finance.usage_inbox (org_id, reservation_id) WHERE state = 'pending';

CREATE TABLE finance.receipt_outbox (
  receipt_id uuid PRIMARY KEY,
  region text NOT NULL,
  category text NOT NULL,
  fact_id text NOT NULL,
  org_id uuid NOT NULL,
  transaction_id uuid NOT NULL REFERENCES finance.journal_transaction (transaction_id),
  rated_microusd bigint NOT NULL CHECK (rated_microusd >= 0),
  pricing_version text NOT NULL,
  payload jsonb NOT NULL,
  dispatch_state text NOT NULL CHECK (dispatch_state IN ('pending', 'dispatched')),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (region, category, fact_id)
);
CREATE INDEX receipt_outbox_pending_idx
  ON finance.receipt_outbox (region, category, created_at)
  WHERE dispatch_state = 'pending';

CREATE TABLE finance.provider_effect (
  effect_id uuid PRIMARY KEY,
  org_id uuid NOT NULL REFERENCES control.organization (org_id),
  kind text NOT NULL CHECK (kind IN (
    'customer_create', 'checkout_session_create', 'payment_intent_off_session',
    'refund_create', 'tax_calculation_create', 'tax_transaction_create'
  )),
  state text NOT NULL CHECK (state IN (
    'prepared', 'dispatched', 'succeeded', 'failed', 'outcome_unknown', 'manual_review'
  )),
  intent_hash bytea NOT NULL CHECK (octet_length(intent_hash) = 32),
  request_json jsonb NOT NULL,
  amount_microusd bigint NULL CHECK (amount_microusd >= 0),
  tax_microusd bigint NULL CHECK (tax_microusd >= 0),
  deadline_at timestamptz NOT NULL,
  provider_object_id text NULL,
  provider_request_id text NULL,
  provider_status text NULL,
  failure_code text NULL,
  decline_code text NULL,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  first_dispatch_at timestamptz NULL,
  resolved_at timestamptz NULL,
  transaction_id uuid NULL REFERENCES finance.journal_transaction (transaction_id),
  revision bigint NOT NULL DEFAULT 0,
  UNIQUE (org_id, kind, intent_hash)
);
CREATE UNIQUE INDEX provider_effect_object_uniq
  ON finance.provider_effect (provider_object_id) WHERE provider_object_id IS NOT NULL;
CREATE UNIQUE INDEX provider_effect_one_open_charge
  ON finance.provider_effect (org_id)
  WHERE kind = 'payment_intent_off_session'
    AND state IN ('prepared', 'dispatched', 'outcome_unknown');

CREATE TABLE finance.provider_event_inbox (
  provider_event_id text PRIMARY KEY,
  event_type text NOT NULL,
  object_id text NOT NULL,
  org_id uuid NULL,
  provider_api_version text NOT NULL,
  created_at_provider timestamptz NOT NULL,
  raw_body_sha256 bytea NOT NULL CHECK (octet_length(raw_body_sha256) = 32),
  schema_id text NOT NULL,
  normalized jsonb NOT NULL,
  applied_state text NOT NULL CHECK (
    applied_state IN ('applied', 'ignored_unsupported', 'quarantined')
  ),
  transaction_id uuid NULL REFERENCES finance.journal_transaction (transaction_id)
);
CREATE INDEX provider_event_object_time_idx
  ON finance.provider_event_inbox (object_id, created_at_provider DESC);

CREATE TABLE finance.billing_account (
  org_id uuid PRIMARY KEY REFERENCES control.organization (org_id),
  provider_customer_id text NULL UNIQUE,
  default_payment_method_id text NULL,
  auto_topup_enabled boolean NOT NULL DEFAULT false,
  auto_topup_threshold_microusd bigint NOT NULL DEFAULT 0 CHECK (auto_topup_threshold_microusd >= 0),
  auto_topup_amount_microusd bigint NOT NULL DEFAULT 10000000 CHECK (auto_topup_amount_microusd >= 10000000),
  spend_cap_microusd bigint NULL CHECK (spend_cap_microusd > 0),
  state text NOT NULL CHECK (state IN ('active', 'payment_hold', 'dispute_hold', 'closed')),
  state_reason text NULL,
  tax_address jsonb NULL,
  revision bigint NOT NULL DEFAULT 0,
  CHECK (NOT auto_topup_enabled OR default_payment_method_id IS NOT NULL),
  CHECK ((state = 'active') = (state_reason IS NULL))
);

CREATE TABLE finance.statement (
  statement_id uuid PRIMARY KEY,
  org_id uuid NOT NULL REFERENCES control.organization (org_id),
  period text NOT NULL CHECK (period ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'),
  state text NOT NULL CHECK (state IN ('building', 'issued')),
  opening_microusd bigint NOT NULL,
  closing_microusd bigint NOT NULL,
  content_sha256 bytea NULL CHECK (content_sha256 IS NULL OR octet_length(content_sha256) = 32),
  object_key text NULL,
  issued_at timestamptz NULL,
  UNIQUE (org_id, period),
  CHECK (issued_at IS NULL OR period < to_char(issued_at, 'YYYY-MM'))
);

CREATE TABLE finance.provider_cost_fact (
  source text NOT NULL,
  source_row_id text NOT NULL,
  period text NOT NULL,
  service text NOT NULL,
  region text NOT NULL,
  cost_microusd bigint NOT NULL CHECK (cost_microusd >= 0),
  PRIMARY KEY (source, source_row_id)
);
