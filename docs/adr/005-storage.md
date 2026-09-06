# ADR-005: Use one serving node and make its durability boundary explicit

Status: Proposed. Date: 2026-09-06. Accept only with the stated availability trade-off.

## Context

Brain's current implementation uses a local canonical journal plus server metadata, request
claims, host registrations and admitted artifacts. Aex additionally needs transactional
account ownership. Putting one store in a distributed database would not distribute all of
these responsibilities or establish a new owner for an interrupted session.

## Decision

Use one serving node, one Brain writer and one Aex service. Brain retains its existing local
format. Aex uses SQLite on a local filesystem backed by persistent block storage. Use durable
transactions (including appropriate synchronous settings), foreign keys and schema migrations.
Keep blocking SQLite work off async executor threads through the chosen library's supported
bounded execution mechanism. No cloud SDK enters Brain's core.

Both stores occupy separately permissioned paths on the same persistent data volume so a
consistent backup can cover the unit. Brain's directory is not merely `sessions/`: metadata
and its key, request claims, hosts, admitted artifacts and the format marker also matter.
Transient sockets and live worker state are not recovered execution. Document the actual
versioned backup procedure rather than inventing a second Brain storage layout.

Use Brain's existing data-directory lock and a supervised stop-before-start deployment.
Platform retains and attaches the volume to only one serving owner. Do not add a distributed
lease/consensus system to a single-node design. A second writer must fail to start; an old
owner must be stopped/fenced before the retained disk is attached to a replacement host.

Choose SQLite because the proposed service has one local product-state writer and no paid
ledger or shared control plane yet. PostgreSQL becomes appropriate when independent Aex
instances must concurrently write shared product state. Keep SQL ownership cohesive, but
do not build two adapters or promise a migration will require only changing a connection URL.

Do not deploy this SQLite database on a network filesystem. SQLite documents sync/locking
caveats for that topology, and WAL relies on same-host shared memory. See
[network guidance](https://www.sqlite.org/useovernet.html) and [WAL documentation](https://www.sqlite.org/wal.html).
The node's local filesystem over a block device is distinct from opening a database over NFS.

## Customer-visible durability

| Failure | Proposed guarantee and limitation |
| --- | --- |
| Aex/Brain process restart, data volume intact | Acknowledged durable state survives; interrupted turns are reported and effects are not replayed |
| Host loss, data volume intact and safely reattached | Operator restores the same state; service is unavailable during recovery |
| Data-volume loss or loss of its availability zone | Restore from a completed backup; writes after that backup may be lost |
| Worker or customer host-process loss | History survives; live execution/resources are not reconstructed from history |
| Region loss | No cross-region service or recovery guarantee in MVP |

Recovery-point and recovery-time targets are selected and measured in Platform before
onboarding; there is no invented uptime SLA. A single-node preview can scale vertically
within a tested envelope, but does not claim automatic horizontal scaling or failover.

Backups must be application-consistent across product ownership and Brain state. The first
procedure may drain admission and stop writers briefly, flush, capture the whole data
volume, then resume. A copied live SQLite main file alone is not a valid backup procedure.
Accept the maintenance cost explicitly and test restoration of a completed backup.

Session deletion removes active access and invokes Brain cleanup. Its current credential
log records logical forgetting; do not promise immediate physical erasure of old encrypted
entries or completed backups. The retention policy covers both. Before reopening a restored
backup, apply post-backup revocations/deletions from protected operator records, or rotate
credentials and keep the affected resources inaccessible pending reconciliation. Recovery
must not silently resurrect revoked access or intentionally deleted customer data.

## Consequences and reversal

This avoids an external journal implementation, shared ownership protocol and cloud database
in MVP. It accepts maintenance interruptions and a bounded backup data-loss window. Select
an HA design instead if those limits invalidate the first customer journey.

Horizontal scale later requires a durable session directory, routing for sessions and hosts,
single-writer ownership/fencing, drain/transfer semantics and an explicit durable-storage
strategy for all Brain auxiliary state. These are post-MVP requirements, not decided designs.

## Acceptance evidence

Test second-writer refusal, kill/restart, full disk, interrupted creation/deletion, retained
volume recovery, consistent backup restoration and post-restore access revocation. Assert
preservation of acknowledged state within each documented boundary, not an unavailable
guarantee after complete disk loss.
