-- aex-migration: tx=yes destructive=no phase=contract
-- The launch identity surface accepts Google only. PostgreSQL validates the
-- replacement constraint against every existing row before this transaction
-- can commit, so a legacy GitHub identity stops the release without deleting
-- or rewriting customer authority.

ALTER TABLE identity.external_identity
  DROP CONSTRAINT ext_provider_ck,
  ADD CONSTRAINT ext_provider_ck CHECK (provider IN ('google'));
