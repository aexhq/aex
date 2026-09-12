# Production preview: accounts, PostgreSQL and thin SDK

Accepted 7 September 2026. Supersedes the initial invite-only, SQLite and curated-only
MVP scope where they conflict. Earlier ADRs remain the rationale for ownership,
fail-closed admission and unchanged Brain semantics.

The website runs on Vercel and verifies Google identities. Dashboard navigation is
exactly API keys, Docs, and Account / Billing / Usage. Accounts are self-service up to
the configured preview capacity. Dashboard credentials cannot be confused with workload
keys; only the former can manage keys. Hosting is free preview with customer model keys.

Aex owns product metadata in PostgreSQL: account identity, key verifiers, ownership,
claims, admission reservations and usage. Brain owns execution and its EBS data. Aex
never changes Brain's protocol or puts Brain's journal in the product database.

The SDK extends and re-exports Brain's SDK, adding default hosted transport and account
methods. Applications run their own script and event handling. Brain-compatible Wasm
Agentloops and Tools can run in hosted Brain; hostEnv functions stay in the application.
Arbitrary HTTP Environments and native secret, filesystem or networking grants are not
enabled. Artifact uploads reserve bounded account storage before compilation.

There is one production environment in us-east-1: EC2 with retained EBS, private
Single-AZ RDS PostgreSQL, TLS ingress and Vercel. All CI checks gate releases. There is
no deployed dev or staging environment. The private Platform repository owns the
immutable release tuple, provisioning, paired backups and recovery procedure.
