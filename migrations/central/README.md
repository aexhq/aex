# `migrations/central/`

Ordered SQL migrations for the central Aurora PostgreSQL cluster, applied only
by `central-schema-admin`, which holds the sole DDL and grant identity.

Expand/contract discipline applies: a migration must be safe against the
previous and the next application version.
