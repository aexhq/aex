-- aex-migration: tx=yes destructive=no phase=expand
-- GitHub is a supported sign-in authority. Existing Google links remain valid;
-- the expanded check admits the second provider without rewriting identity.

ALTER TABLE identity.external_identity
  DROP CONSTRAINT ext_provider_ck,
  ADD CONSTRAINT ext_provider_ck CHECK (provider IN ('github', 'google'));
