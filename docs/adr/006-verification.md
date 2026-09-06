# ADR-006: Gate release on real integration, bounded resources and recovery

Status: Proposed. Date: 2026-09-06.

## Context

Performance, scalability and maintainability are requirements, not reasons to prebuild a
distributed platform. Brain's historical benchmark figures do not establish performance of
this rewrite, its current journal commit path, or the proposed hosted substrate.

## Decision

Keep one authoritative implementation for contracts, configuration and behavior. Public
contract changes regenerate their views; a Brain upgrade pins a matching source/SDK/image
set and runs hosted compatibility journeys. Aex does not infer compatibility from a package
name, a moving branch or a `latest` image tag.

Tests accompany the failure they catch. The initial release runs every required suite;
there is no test-skipping escape hatch. Local checks aid development; green CI and hosted
staging evidence gate release. A missing credential for a required live check is a blocked
check, not a pass or a reason to silently substitute a mock.

| Concern | Required evidence |
| --- | --- |
| Contracts and maintainability | Format/lint, all Rust tests and doctests, generated-contract diff, clean public build and executable examples |
| Brain compatibility | Published SDK against Aex and real Brain workers; curated admission, application Tool round, lifecycle and host reconnect |
| Tenant boundary | Two-account matrix for all supported route classes; key revocation, host substitution, same-key concurrency and unsupported routes |
| Durable correctness | Fault injection around claims, ownership and deletion; no re-executed unknown effects; second writer rejected; restore tested |
| Streaming | Real ingress, long idle connection, slow subscriber, disconnect/reconnect from committed sequence, bounded memory |
| Capacity | Concurrency/retention/context-size sweeps with errors and rejections included; no unbounded queues, state or disk growth |
| Supply and deployment | Dependency/security scans, container smoke, infrastructure validation and plans, immutable staging canary and rollback proof |

Performance measurements separate gateway overhead, durable acknowledgement, worker queue,
Tool execution and model-provider time. Use the same machine, pinned artifacts and request
mix for direct Brain and Aex comparisons. Report cold and warm populations separately and
include failure rates. A model-free fixture isolates runtime overhead; a real provider and
host Tool prove the customer journey.

Before M5, the launch plan must record numerical acceptance budgets for gateway latency,
active turns, stored sessions/bytes, simultaneous streams, memory/disk headroom and recovery.
These are deployment/workload-specific values. Set them before the final measurement and
do not relax them after a failed result without a recorded scope decision. Resource-safety
properties and no cross-account access are mandatory regardless of those numbers.

The live feed is observation; committed Brain records are the durable sequence. Logs contain
redacted request/account/session correlation and actionable failure categories, not transcripts
or credentials. Existing Brain telemetry is reused where sufficient. No duplicated durable
analytics/event pipeline is required for launch.

Build immutable source-linked artifacts once, deploy them to staging, then promote those
same artifacts. Test data-format compatibility before rollback. If new writes make old
software incompatible, stop serving and use the documented forward repair or restore path;
switching an image digest alone is not a data rollback.

## Alternatives and consequences

Mocks alone miss worker, stream, filesystem and provider boundaries. Large benchmark suites
without a launch workload produce numbers without a decision. A first-principles distributed
design would add complexity before evidence identifies a capacity or availability bottleneck.

One public service and one pinned Brain server keep the dependency graph inspectable.
Private deployment evidence can contain instance choices and cost measurements without
making public tests depend on a private account.

## Acceptance and reconsideration

CI commands are documented once when build tooling exists. M0 is a spike, not a declaration
that these gates have passed. Release records distinguish measured, unmeasured and blocked
work. Add a new gate when a supported feature introduces a new failure boundary, rather than
rechecking settled properties through another redundant layer.
