# Operations and recovery

Use `aex-server operate --request JSON` with `AEX_OPERATOR_TOKEN` in the process environment.
The [generated operator schema](generated/operator.schema.json) defines commands. Never proxy
this loopback listener through public ingress. On the AWS host use `docker exec aex-control
 aex-server operate --request JSON`; the operator port is not published from the container.

## Identity and reconciliation

`create_account`, `issue_key`, `revoke_key`, `suspend_account` and `resume_account` manage access.
`inspect` lists accounts and unresolved claims without credentials or transcripts. `forget_host`
retires an Aex host registration; Brain expires unused registrations under its own lifecycle.

`drain` closes customer access and streams. Wait for zero in-flight requests before reconciliation.
Every restart starts drained. `resume` reopens after dependency/storage checks. Record revocations
and deletions in protected operator records outside the serving volume. Reapply post-backup
changes after restoring an older backup, before resume.

For ambiguous creation: drain, let Brain creation settle, inspect pending claims and `orphans`,
use `delete_orphan` only for identified unowned sessions, then `resolve_create` to release the
allocation. Never infer ownership from timing or request similarity. For an uncertain turn,
stop/restart Brain if a request may remain in flight, then use `reconcile_turn` while drained.
It releases capacity only for a non-running Brain state and never reruns work.

## Storage

Aex and Brain access only their own directories. A trusted operator measures file lengths in
Brain's versioned session directories and reports through the local operator API. It reads no
session contents or credential metadata. Shared metadata/artifacts use whole-volume headroom
and bounded record counts rather than per-tenant attribution.

Turns reserve retained-byte capacity transactionally. A fresh scan started after the last
completed write can replace reservations with measured bytes. Active/uncertain work cannot
have its allocation reset by a scan. Stale reports or insufficient disk space deny new work.
`maintain` tombstones expired sessions, ends them and confirms deletion. It can interrupt old
sessions; failed cleanup remains inaccessible and is retried on subsequent maintenance.

The byte reserve is an admission allowance, not a filesystem quota. In-flight growth and the
metering interval must fit measured headroom. Before onboarding, saturation tests must validate
these settings with the pinned Agentloop and Brain limits. Do not advertise a hard physical-disk
quota or arbitrary hostile-code hosting.

## Durability

Stop admission and both writers for consistent whole-volume backup. Preserve SQLite WAL state
and Brain metadata/key, claims, hosts, artifacts, format marker and journals. Runtime sockets
under `run/` are transient. Record compatible source revisions and image digests with backups.

Stop the previous writer before moving a retained volume. Disk loss requires a completed backup
and may lose later writes. Live execution is not reconstructed. Reconcile post-backup access
revocations and deletions before reopening. Logical deletion does not erase historical backups.
Rollback requires data-format compatibility; changing an image alone is not a data rollback.

## Verification

`tools/check.sh` runs strict Rust checks/tests, generated-contract verification, npm audit and
meter tests. `tools/journeys.sh` runs the published SDK, real Linux workers, customer Tools,
isolation, revocation, restart and consistent file restore. It reports a direct/gateway read
baseline, 1/2/4/8 concurrent-turn admission and unread-subscriber exhaustion. It also verifies
second-writer rejection, consecutive turns, retention and credential redaction.
`tools/image-smoke.py` verifies the non-root container and writable data path.

`tools/live-local.py --workspace-env /workspace/.env.dev` runs a real-provider check with explicit
`AEX_TEST_SERVER`, `BRAIN_TEST_SERVER`, `BRAIN_TEST_WORKER` and `BRAIN_TEST_REFERENCE_AGENTLOOP` paths.
`tools/live-check.mjs` can target deployed Aex using environment-supplied test credentials.
Missing credentials or provider failure fail this release check; mocks do not replace it.
