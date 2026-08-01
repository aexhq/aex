-- aex-migration: 20260801000400 finance_roles_and_schema | tx=yes | destructive=no | phase=baseline

CREATE SCHEMA IF NOT EXISTS schema_admin;
CREATE SCHEMA IF NOT EXISTS finance;

DO $$
DECLARE role_name text;
BEGIN
  FOREACH role_name IN ARRAY ARRAY[
    'aex_finance_api',
    'aex_finance_ingest',
    'aex_finance_settlement',
    'aex_finance_reconcile',
    'aex_receipt_dispatcher',
    'aex_provider_cost'
  ] LOOP
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = role_name) THEN
      EXECUTE format('CREATE ROLE %I NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT', role_name);
    END IF;
  END LOOP;
END
$$;

REVOKE ALL ON SCHEMA schema_admin FROM PUBLIC;
REVOKE ALL ON SCHEMA finance FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA schema_admin REVOKE ALL ON TABLES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA finance REVOKE ALL ON TABLES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA finance REVOKE ALL ON SEQUENCES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA finance REVOKE ALL ON FUNCTIONS FROM PUBLIC;

CREATE TABLE IF NOT EXISTS schema_admin.backfill_cursor (
  migration_version bigint PRIMARY KEY,
  cursor text NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now()
);
