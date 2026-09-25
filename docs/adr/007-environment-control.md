# ADR-007: Compose Brain Environment control within hosted resource authority

Status: Accepted. Date: 2026-09-25.

Brain separates durable agent state from execution and exposes one scoped Environment control
interface to Tools, Agentloops and Environments. The application explicitly chooses automatic
or manual lifecycle when creating a session. Whether a model sees an `env` Tool is an independent
application choice; official Tools receive no special authority.

Aex forwards owner and Host control requests after account and session ownership checks.
Brain owns instance identity, lifecycle, extension grants and journal observations. Model calls
through the generic Host interface retain the same hosted budget admission as dedicated calls.
No second Environment state machine belongs in Aex.

Brain templates permit dynamic instances within authority fixed at creation. Aex's current
managed-compute reservation and approved HTTP binding each cover one declared resource, so
hosted bindings cannot authorize templates or replacement methods. Caller-operated Host
Environments remain free to manage their own instances. Adding paid dynamic instances requires
an admission and settlement contract before permitting their effects, rather than treating the
model's control grant as a spending grant.

Provider-specific inspection and resource management remain Environment methods. A universal
inspection abstraction would force unrelated providers into one schema without improving the
hosting boundary. Brain records failures and unknown outcomes; neither layer retries effects
automatically. The retained journal outlives provider failure, but provider files do not acquire
session durability.
