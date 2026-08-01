# `migrations/`

`central/` holds the SQL migrations applied by `central-schema-admin` through
SQLx. `regional/` holds the regional table generation definitions.

Migrations ship as one ordered immutable bundle digest; they are never assembled
at deploy time.
