# Central schema migrations

This directory is one immutable, strictly increasing `YYYYMMDDHHMMSS` SQLx
chain across the `identity`, `control` and `finance` schemas. The first line of
every migration is:

```text
-- aex-migration: tx=<yes|no> destructive=<no|yes> phase=<expand|contract|baseline|data>
```

Three unordered `key=value` fields. The migration's identity is its **filename**,
`<14 digits>_<slug>.sql`, and is deliberately not repeated inside the body.
`aex-release-tool migration bundle` is the gate for that line and for the phase
vocabulary; `central-schema-admin` parses it field for field, and a unit test in
each pins the same four phases, so a file one admits and the other refuses cannot
reach review. A backfill is a *command* driven by `schema_admin.backfill_cursor`,
not a phase — the migration that carries one is `phase=data`.

SQLx executes SQL directly. There is no custom statement splitter, template,
checksum transition, or history rewrite. A `tx=no` migration must use SQLx's
`-- no-transaction` directive, declare a precondition, and ship a sibling
`<version>_<slug>.repair.sql`.

## Privileges live in `grants.toml`, never in a body

A migration creates objects. It grants nothing, revokes nothing, and names no
role. Every privilege in the central database — the database-wide denial,
`CONNECT`, schema `USAGE`, table privileges and function `EXECUTE` — is declared
once in `grants.toml` and rendered to SQL by
`central_schema_admin::grants::GrantSet::render`, which is what
`central-schema-admin grants --check|--apply` reconciles and what the
container-backed suite in `crates/aex-control-aurora/tests/migrations.rs` applies
before it probes the denial matrix. `aex-release-tool migration bundle` refuses a
body containing a `GRANT` or a `REVOKE`, so the split is a release gate rather
than a convention.

`ALTER DEFAULT PRIVILEGES` stays in the body that creates the schema: it is a
property of the schema rather than an entry in an allowlist, and it governs
objects that do not exist yet.

## `bundle.lock.json`

`aex-release-tool migration bundle --out migrations/central/bundle.lock.json`
records each file's digest, length, header and repair sibling, plus the digest of
`grants.toml`. Regenerate it in the same commit as any change here: `graph
verify` reports `migration-bundle-missing` without it, and
`migration-outside-bundle` for a `.sql` file it does not name.

## Ownership

The central identity/control stream owns the SQL bodies in versions `000000`
through `000300`. The central-finance stream owns the runner and the finance
bodies from `000400` on. The runner owner mechanically renamed the peer's invalid
four-digit filenames during the merge; their order and SQL bodies were preserved.
