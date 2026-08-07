-- aex-migration: tx=yes destructive=no phase=expand
-- Add the API-key event to the immutable outbox topic allowlist. The baseline
-- migration remains byte-stable for databases that have already applied it.
ALTER TABLE control.outbox_message
  DROP CONSTRAINT outbox_topic_ck;

ALTER TABLE control.outbox_message
  ADD CONSTRAINT outbox_topic_ck CHECK (topic IN (
    'workspace.provision.requested',
    'workspace.delete.requested',
    'account.state.changed',
    'invitation.email.requested',
    'api_key.created',
    'authorization.epoch.changed',
    'authorization.signing_key.published'));
