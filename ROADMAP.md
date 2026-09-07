# Aex roadmap

Status: scope accepted 2026-09-07; implementation and release evidence tracked below.
The [MVP ADRs](docs/adr/README.md) explain the architecture. Post-MVP entries are options
with admission criteria, not architecture decisions or delivery promises.

## Product boundary

Host Brain reliably for developers who want an agent runtime without operating it.
Brain remains the source of session semantics; Aex owns the customer boundary around it.

The proposed first customer journey is: obtain an operator-issued API key, use the Brain
TypeScript SDK against Aex, select a supported Agentloop and model, attach application Tools,
create a session, send work, observe and reconnect, inspect history, cancel or end, and delete.
Another account cannot access any of those resources.

### Accepted MVP scope

1. The first release is an invite-only developer preview, with manual account onboarding.
2. Customers supply model keys. Aex does not sell model credits or collect payments yet.
3. Hosted execution admits a small curated Component set; custom Tools run in the customer's
   process through Brain's existing `hostEnv` transport. There is no managed shell sandbox.
4. One serving node and a documented maintenance/recovery window are acceptable initially.
5. The public repository contains the reusable hosting implementation. Platform supplies
   private deployment settings and commercial choices through configuration.

If paid self-service, hosted shell execution, or automatic failover is essential to the
first customer journey, move the corresponding work into MVP before estimating it.
They are separate scope changes; “official host” alone does not settle them.

## Scope

| MVP | Deferred |
| --- | --- |
| Accounts, API keys, key revocation, account suspension | Signup/login UI, organizations, roles, SSO |
| Account-owned sessions and host registrations | Shared sessions, cross-account sharing |
| Curated Agentloop admission and customer application Tools | Arbitrary hosted Components, shell sandbox fleet, custom images |
| Supported provider catalogue and per-session customer keys | Aex-funded inference, pricing, credits, payments, refunds |
| Brain lifecycle, transcript, committed Events and live SSE | A second event system, workflow retries, mutable placement |
| Bounded admission, storage and stream usage | Fair scheduling across arbitrary hostile workloads |
| One serving node, persistent disk, backup/restore and interruption semantics | Automatic failover, horizontal session placement, multi-region |
| Brain SDK compatibility, executable quickstart and operator commands | Separate Aex session SDK, general customer CLI, dashboard |
| CI, deployment smoke, resource measurements and recovery proof | Premature service decomposition or a plugin framework |

## MVP milestones

Each milestone produces an executable slice. Estimates follow M0 and a settled customer
journey; calendar dates now would hide the unknown integration work. M1–M4 depend on M0;
M5 requires all of them. Security and recovery tests ship with their owning slice.

### M0 — Settle scope and prove the Brain seam

- Agree the five assumptions above and the launch workload and recovery envelope.
- Select one public, CI-green immutable Brain revision and compatible official Agentloop.
- Run the existing Brain SDK through a small Rust gateway against an unchanged Brain server:
  curated Component admission, host registration, create, one Tool call, Events, end and delete.
- Prove separate account identity for identical idempotency keys and host ownership.
- Inject failure around Brain creation and Aex ownership commit. Determine exactly which
  outcomes can be recovered with the current Brain API and which remain ambiguous.
- Measure direct Brain versus the gateway. Confirm that HTTP composition is sufficient
  before considering embedding or an upstream neutral API change.
- Verify disk restart and one-writer behavior on the candidate Linux substrate.

Done: agreed scope, a reproducible seam experiment, a compatibility record, and explicit
resolution of create/host ambiguity. No production provisioning is needed for the first
local experiment. Research evidence stays private; the accepted contract stays public.

### M1 — Ship the authenticated session journey

- Build one `aex-server` service with cohesive identity, session, host, and Brain-client modules.
- Add local transactional product storage, account/key operator commands, and schema migrations.
- Admit only declared public routes; authorize sessions and host bindings by account.
- Preserve Brain request, response, error, event, and lifecycle shapes for supported operations.
- Commit account ownership before returning a newly created session or host registration.
- Scope operation keys by account and operation; preserve uncertainty instead of replaying effects.
- Exercise the full customer journey using the published Brain SDK and a real worker process.

Done: two-account tests cover reads, lists, mutations, host substitution, key reuse and
revocation; restart does not lose ownership of acknowledged resources. Unowned resources
never become public through discovery or guessed identifiers.

### M2 — Bound hosted execution and credential access

- Allow only release-selected Component content and supported model/provider identifiers.
- Accept application Tool definitions while keeping their execution in the registered host.
- Reject customer-selected HTTP Environments and server capability grants in this MVP.
- Set deployment byte, duration, memory, worker and queue limits through Brain configuration.
- Add only Aex-specific account/session/stream admission that Brain does not already supply.
- Bound accepted concurrent turns and retained storage; recover reservations from authoritative
  state after interruption. Test exhaustion without relying on successful model responses.
- Keep customer keys out of product tables, journals, telemetry and request/response logs;
  verify Brain credential persistence and cleanup through the supported API.

Done: a customer cannot execute unreviewed code on Aex, select another account's host,
grant access to server secrets/network/files, or exhaust the entire service through unbounded
requests or retained sessions. Residual shared-process contention is measured and documented.

### M3 — Prove durable operation and cleanup

- Preserve Brain's whole data directory and Aex's product database as one recoverable unit.
- Test process kill, host replacement with retained disk, and restore from a completed backup.
- Verify interrupted and unknown outcomes and ordered event reconnection; never rerun effects
  to make an outage appear successful.
- Complete idempotent deletion with a durable ownership tombstone while cleanup is pending.
- Implement the documented retention policy and storage-full admission behavior.
- Prove revoked/suspended access stays denied after restart and after backup restoration.

Done: a written durability envelope, measured recovery time, successful restore of ownership,
credentials and history, and a cleanup runbook. Backup expiry and restored-deletion handling
are explicit; logical deletion is not advertised as instantaneous erasure from backups.

### M4 — Establish the operational and performance baseline

- Record gateway latency separately from Brain journal/worker time and model/provider latency.
- Measure cold admission, warm creation, message acknowledgement, committed event delivery,
  history reads, reconnect, and a real model plus application-Tool round.
- Sweep concurrent active turns, retained sessions, subscribers and large contexts. Include
  slow clients and saturated workers; record rejections, memory, CPU, disk growth and queue time.
- Use request/account/session identifiers in redacted operational traces. Alert on inability
  to commit, repeated worker failure, exhausted capacity, and failed/stale backups.
- Test the exact production TLS/ingress path with idle and reconnecting SSE connections.

Done: measured capacity and launch admission settings, reproducible regression workloads,
and no unsupported “scalable” or latency claims. Initial numeric hypotheses live in the
private launch plan until measured; required acceptance conditions are in ADR-006.

### M5 — Release the limited preview

- Run all required Rust, contract, SDK journey, Linux worker, container, security and
  deployment checks in CI. Do not omit a suite to obtain a green release.
- Build immutable Aex and Brain artifacts; promote the same artifacts after staging proof.
- Prove deployment, rollback compatibility, backup restoration, and the two-account canary.
- Publish only the tested supported API subset, limitations, quickstart and support route.
- Onboard the agreed initial users and check the first real workload against the capacity budget.

Done: an operator can onboard, diagnose, suspend, restore and remove a customer without
editing tables manually; the documented end-to-end journey works on the deployed release.
Public Git history replacement and repository visibility are separate future publication actions.

## Post-MVP roadmap

Choose the next outcome from actual demand. These tracks are not all sequential commitments.

| Outcome | Trigger | Smallest next slice | Dependencies and exit evidence |
| --- | --- | --- | --- |
| Paid self-service hosting | Users will pay for the proven journey | Managed identity, key management UI, one pricing model and payment flow | Private commercial rules; public billing semantics; duplicate/payment failure tests and support process |
| Aex-funded models | Customers require consolidated inference spend | One funded provider path with admission and replayable receipt accounting | Charge basis, reservations, absent-counter policy, integer amounts, reconciliation and disputed/unknown outcomes settled before charging |
| Managed execution | Customers need Tools to run while their app is offline, or need shell/files | One ordinary remote Environment integration | Sandbox isolation, network authority, secrets, lifecycle, cleanup, capacity and cost proven; no new session engine |
| Horizontal session capacity | Measured single-node limits constrain demand | Durable session directory and static ownership across multiple Brain nodes | Shared product DB, owner routing, host routing, draining and fenced ownership; Brain changes only at neutral interfaces |
| Better availability and recovery | Single-node maintenance or backup data-loss window is unacceptable | Tested node failover and appropriate durable storage | All Brain auxiliary state, credentials, request claims and Environments covered; distributed single-writer proof |
| Customer-hosted extensions | Curated set blocks real workloads | Bounded tenant-owned artifact admission in an isolated execution tier | Compilation isolation, resource fairness, cache/storage quotas and negative isolation tests |
| Team operations and visibility | Multiple users need the same account | Organizations/roles plus a small dashboard | Proven authorization model; reuse API and event cursors |
| Additional SDKs and CLI | Repeated customer integration work warrants it | One demanded client/workflow | Generate Aex-owned contracts; reuse Brain's neutral client behavior |
| Regions, enterprise controls, advanced orchestration | Residency, scale or customer requirements justify it | Scope one concrete requirement | New ADRs at that time; no speculative multi-cloud abstraction now |

Moving product metadata to PostgreSQL does not by itself distribute Brain sessions.
Likewise, adding a remote Environment does not provide automatic session recovery.

## Implementation record - 2026-09-07

The public service, generated configuration/operator contracts, SDK example, operator tooling,
SQLite ownership/claims, bounded admission, storage reporting and CI are implemented. Local
Windows Rust checks and Linux published-SDK/real-worker journeys pass. A real-provider local
hosted session passes. Tests cover tool execution, isolation, stream revocation, restart,
consistent backup-file restoration and unresolved create claims.

M0/M1 have executable implementation evidence. M2/M3 mechanisms and local recovery tests exist;
M4 has a reproducible direct/gateway warm-read baseline, 1/2/4/8 concurrent-turn admission
sweeps with 1 KiB/64 KiB messages, unread-subscriber exhaustion and redacted request traces.
Second-writer rejection and end-before-delete/retention behavior are tested with real processes.
Full M4 saturation and final admission
values remain release gates. M5 is not complete: AWS provisioning, actual TLS/SSE, instance
isolation, EBS reattachment, completed-snapshot restoration, measured RPO/RTO and onboarding are
explicitly held. CI success does not substitute for deployment-dependent gates.

Byte reservations and resource limits remain launch hypotheses. Storage admission is not a
hard filesystem quota. No production availability, capacity or recovery promise is made
before the held acceptance work passes.
