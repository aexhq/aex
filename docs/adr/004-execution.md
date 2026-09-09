# ADR-004: Limit MVP execution to curated Components and customer application Tools

Status: Accepted. Date: 2026-09-07. Product scope accepted; deployment evidence remains required.

## Context

Hosting arbitrary extensions and shell environments creates a separate code-admission,
isolation, networking and capacity problem. Brain supplies capability-restricted workers,
but its roadmap still defers advanced fairness for mutually untrusted extensions. A useful
developer journey can already execute custom functions in the customer's own process.

## Decision

Admit a small release-selected set of official Agentloop Components, with the corresponding
supported implementation descriptors and configuration. Add native Tools only when the
first journey needs them. Custom application Tools use account-owned `hostEnv` registrations.
Curated means reviewed content identity, not a caller-supplied name or publisher label.

A customer can define prompts, Tool schemas, inputs and fixed permitted placements using
Brain's existing API. All such data remains untrusted. The customer cannot introduce a
different hosted implementation, server secret name, writable host path, or network grant.
Brain's Environment-owned grant configuration and deployment ceilings remain authoritative
inside the Environment. Aex rejects nonempty native configuration, including filesystem,
network, and secrets; removing extension dependency declarations does not grant access.

No customer-selected HTTP Environment URL, provider base URL or arbitrary native-network
grant is accepted in MVP. The server uses an operator-selected supported provider catalogue.
Application Tools remain able to use the customer's network under the customer's authority.
This narrows Aex's outbound surface rather than building a general SSRF filtering service.
URL syntax validation alone is not a hosted network boundary; see
[OWASP's SSRF guidance](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html).

Customers supply the model key per session using Brain's existing contract. Brain retains
the credential for model execution and restart; extensions do not receive it. Aex forwards
the credential over its private channel without persisting it in product tables or logging
request bodies. Verify that error paths and traces do not expose it. MVP has no Aex-funded
models, wallet, pricing engine, usage-charge ledger or payment provider integration.

Brain's current metadata encryption stores its master key in its data directory. That
protects credential representation and session binding, not theft of the entire directory.
Platform therefore protects the complete disk, backups and access to both ciphertext and
key. Do not claim separate external key custody without implementing it through Brain's
credential interface. Deletion and backup retention are governed by ADR-005.

## Resource admission

Use Brain's injected byte, duration, memory, fuel, queue and worker limits. Do not add a
second set of the same limits in Aex. Aex adds account-specific retained-session/storage,
concurrent-turn and stream admission because Brain does not know customer allocations.

Reserve scarce capacity before accepted work; concurrent requests cannot race past an
account's allowance. Keep reservations conservative when outcomes are unknown. Committed
terminal state or explicit operator resolution releases them; lossy telemetry does not.
Test restart reconciliation so limits neither reset to unlimited nor stay stuck forever.

Count session objects when they reserve retained storage; do not cap Tool definitions or
Environment names merely because they are countable. A retained-byte budget and nonzero
disk headroom policy catch disk exhaustion; thresholds are deployment data chosen from M4.
Do not invent a per-tenant scheduler until admitted workloads require one.

## Alternatives and consequences

Customer model keys minimize commercial scope, but introduce credential-custody responsibility
and may add onboarding friction. Customers still pay their provider and Aex still needs
compute admission. This is a limited preview, not a validated revenue model.

A managed remote Environment may be the right first product if customers primarily want a
hosted coding agent. If so, move that one integration and its isolation/cost tests into MVP.
Do not silently promise that `hostEnv` keeps running after the customer's process stops.

Public arbitrary Component uploads would require compilation isolation, artifact quotas and
resource fairness beyond a curated release. Wasm capability isolation alone is not evidence
of bounded noisy-neighbor effects or an additional hardware security boundary.

## Acceptance and reversal

Run the real curated worker and custom host Tool journey. Deny changed Component bytes,
unsupported configuration/grants, another account's host, hostile inputs and unbounded
streams. Kill a worker and disconnect the host; observe ordinary Brain failure/uncertainty
without hidden retries. Reconsider scope when customer work demonstrably requires managed
execution or Aex-funded inference.
