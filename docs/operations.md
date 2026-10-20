# Operations and recovery

Use `aex-server operate --request JSON` with `AEX_OPERATOR_TOKEN`. The [generated operator schema](generated/operator.schema.json) defines commands. Keep its literal-loopback listener private.

`create_account`, `issue_key`, `revoke_key`, `suspend_account` and `resume_account` manage access. `inspect` shows unresolved product claims. `forget_executor` retires an executor registration. Revocations apply to requests and ongoing event streams.

Every restart begins drained. Before `resume`, verify Brain readiness, controller health and fresh storage metering. `drain` closes admission; wait for zero in-flight requests. Preserve post-backup revocation/deletion records outside the serving volume and reapply them before resuming a restored pair.

## Reconcile evidence

Creation claims and accepted turn receipts survive a gateway restart. For ambiguous creation, drain, inspect `orphans`, remove only proven unowned sessions with `delete_orphan`, and resolve the product claim with `resolve_create`. Never infer ownership from timing. `reconcile_turn` and `maintain` release capacity on durable terminal or explicit unknown evidence without replaying work. Idle status alone does not establish that a request was rejected.

Model receipts use opaque journal cursors committed atomically with usage. Physical attempts supersede logical aggregates rather than adding them twice. Prices apply at request start. Missing usage stays unknown; reported lower bounds can be charged and pending estimates remain separate. Financial records survive content deletion. See [billing](billing.md).

A sandbox allocation intent is durable before dispatch and tagged at the provider. Reconcile unknown allocation identities before any funding release. If the start time is unknown, terminating a discovered resource is insufficient evidence for a zero charge. Record provider-supported cumulative usage through the operator's `record_metered_usage` command; terminal evidence can settle a revoked grant.

Session deletion first closes work, reconciles turns and usage, revokes managed grants and waits for external cleanup. Then it removes transient media, attachment objects, Brain content and cached private payloads. A 409 is pending cleanup, not successful deletion. Repeat the same deletion after reconciliation.

## Paired backup and restore

PostgreSQL owns product inventory, billing, cursor receipts and the stateless application controller's private journal. Brain owns native checkpoints, encrypted credentials and archived session history. The sandbox controller owns its retained allocation journal. Preserve all three together with the matching release configuration and encryption keys.

Drain admission, stop Brain and retained controller writers, and quiesce the stateless application controller through its private `/control/quiesce` endpoint using `AEX_TOOL_TOKEN`. Wait for active actions, completion writes and maintenance to reach zero before dumping PostgreSQL and archiving retained files. Keep the gateway available until those private writes finish. Resume the controller and owning runtime before uploading the completed encrypted pair.

Verify restores in an isolated database and volume using the pinned runtime, comparing account/session/usage inventory and continuing a retained session. Never restore only PostgreSQL, only Brain, or a newly generated credential key. After new customer writes, rollback requires the matching paired backup.

## Brain 2 cutover

The migration retains historical product/financial rows and explicitly marks old sessions `retained`; it provides no old execution routing or importer. Before migration, take a verified paired backup and reconcile active sessions, pending claims, nonterminal model usage and open turn/sandbox reservations. Migration 0012 refuses those unresolved states. Initialize a fresh Brain 2 data directory with a new deployment-owned credential key; keep the original pair intact for historical recovery.

Deployment-specific images, service settings, commercial prices, credentials and production evidence belong to private Platform. Public implementation and CI remain independently reproducible.
