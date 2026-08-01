# Central schema migrations

This directory is one immutable, strictly increasing `YYYYMMDDHHMMSS` SQLx
chain across the `identity`, `control`, and `finance` schemas. The first line of
every migration is:

```text
-- aex-migration: <version> <slug> | tx=<yes|no> | destructive=<no|yes> | phase=<expand|backfill|switch|contract|baseline>
```

SQLx executes SQL directly. There is no custom statement splitter, template,
checksum transition, or history rewrite. A `tx=no` migration must use SQLx's
`-- no-transaction` directive, declare a precondition, and ship a sibling
`<version>_<slug>.repair.sql`.

The central-finance stream owns the runner and versions `00100`, `00400`,
`00500`, and `00600`. The central identity/control stream owns the bodies in
the reserved `00200` and `00300` slots.
