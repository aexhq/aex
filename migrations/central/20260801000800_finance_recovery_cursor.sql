-- aex-migration: tx=yes destructive=no phase=expand

-- A bounded reconciliation page must not let one permanent unresolved effect
-- starve every later effect. The cursor is advisory and replay-safe: it moves
-- only after a page was processed, and wrapping it merely revisits old rows.
CREATE TABLE finance.reconcile_cursor (
  duty text PRIMARY KEY CHECK (duty IN ('provider_effect')),
  last_effect_id uuid NOT NULL,
  updated_at timestamptz NOT NULL
);

