-- 0001_bootstrap.sql — database-wide denial and the group roles.
--
-- Two rules this file exists to establish.
--
--   1. Nothing is granted by default. `PUBLIC` loses every database privilege
--      and the `public` schema is dropped outright, so an object created
--      without an explicit schema is an error rather than a world-readable
--      table.
--   2. Every application role is a `NOLOGIN` group. `central-schema-admin`
--      creates the matching `*_login` roles and attaches their Secrets Manager
--      passwords, so no credential value ever enters a migration file, and the
--      grants stay in version control where they can be reviewed.
--
-- There are no extensions. Identifiers and secrets are generated in Rust,
-- emails are normalized in Rust, and a `CHECK` refuses anything else — so the
-- database needs neither `pgcrypto` nor `citext`, and the two layers cannot
-- disagree about what a normalized value is.

REVOKE ALL ON DATABASE aex FROM PUBLIC;

DROP SCHEMA IF EXISTS public CASCADE;

-- Identity and control, this stream's roles.
CREATE ROLE aex_identity_api   NOLOGIN;
CREATE ROLE aex_authz          NOLOGIN;
CREATE ROLE aex_control_api    NOLOGIN;
CREATE ROLE aex_control_worker NOLOGIN;

-- Finance, the peer stream's roles. They are created here so that
-- `0006_cross_schema_grants.sql` has something to grant to, and so that the
-- whole role inventory is readable in one file.
CREATE ROLE aex_finance_api        NOLOGIN;
CREATE ROLE aex_finance_ingest     NOLOGIN;
CREATE ROLE aex_finance_settlement NOLOGIN;
CREATE ROLE aex_finance_reconcile  NOLOGIN;
