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

The central identity/control stream owns the unchanged SQL bodies in versions
`00000` through `00300`. The central-finance stream owns the runner and the
finance bodies in `00400` through `00600`. The runner owner mechanically
renamed the peer's invalid four-digit filenames during the merge; their order
and SQL bodies were preserved.
