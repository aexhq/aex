-- aex-migration: tx=yes destructive=yes phase=contract
-- Remove the retired non-browser credential ceremony after every runtime and
-- grant has stopped reading or writing it.

DROP VIEW identity.credential_pepper_usage;
DROP TABLE identity.device_authorization;
DROP TABLE identity.account_token;

CREATE VIEW identity.credential_pepper_usage AS
SELECT pepper_version, count(*) AS live FROM (
            SELECT pepper_version FROM identity.dashboard_session WHERE revoked_at IS NULL
  UNION ALL SELECT pepper_version FROM identity.email_challenge   WHERE consumed_at IS NULL
) s GROUP BY pepper_version;
