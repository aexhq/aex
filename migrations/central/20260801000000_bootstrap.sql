-- aex-migration: tx=yes destructive=no phase=baseline
-- 20260801000000_bootstrap.sql — the group roles, and no privileges at all.
--
-- Two rules this file exists to establish.
--
--   1. Nothing is granted by default. The `public` schema is dropped outright,
--      so an object created without an explicit schema is an error rather than
--      a world-readable table, and `grants.toml` revokes every remaining
--      database privilege from `PUBLIC` — `CONNECT` included — before handing
--      any of it back, per role and never to `PUBLIC`.
--   2. Every application role is a `NOLOGIN` group. `central-schema-admin`
--      creates the matching `*_login` roles and attaches their Secrets Manager
--      passwords, so no credential value ever enters a migration file, and the
--      privileges stay in `grants.toml` where they can be reviewed as one
--      document.
--
-- This file therefore revokes and grants nothing. The database-wide denial is
-- `[database] revoke_public` in `grants.toml`, which is also where the
-- qualifying per-role `CONNECT` lives: a revoke and the grant that survives it
-- belong in one document, and the database name is a deployment fact rather
-- than a literal a migration body can hard-code.
--
-- There are no extensions. Identifiers and secrets are generated in Rust,
-- emails are normalized in Rust, and a `CHECK` refuses anything else — so the
-- database needs neither `pgcrypto` nor `citext`, and the two layers cannot
-- disagree about what a normalized value is.

DROP SCHEMA IF EXISTS public CASCADE;

-- Identity and control, this stream's roles.
CREATE ROLE aex_identity_api   NOLOGIN;
CREATE ROLE aex_authz          NOLOGIN;
CREATE ROLE aex_control_api    NOLOGIN;
CREATE ROLE aex_control_worker NOLOGIN;

-- Finance, the peer stream's roles. They are created here so the whole role
-- inventory is readable in one file; `20260801000400` creates the two the
-- finance stream added later, and `grants.toml` names all ten.
CREATE ROLE aex_finance_api        NOLOGIN;
CREATE ROLE aex_finance_ingest     NOLOGIN;
CREATE ROLE aex_finance_settlement NOLOGIN;
CREATE ROLE aex_finance_reconcile  NOLOGIN;
