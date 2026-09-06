# MVP architecture decision records

All records below are **Proposed**, dated 2026-09-06. Owner requirements are identified
separately from recommendations. Acceptance requires discussion; a decision's acceptance
does not mean its implementation is complete.

| ADR | Decision boundary |
| --- | --- |
| [001 — Ownership and repository structure](001-ownership.md) | Brain, public Aex, private Platform; module boundaries |
| [002 — Compose the Brain server over HTTP](002-brain-composition.md) | SDK reuse, routing and integration uncertainty |
| [003 — Account ownership and operation consistency](003-tenancy.md) | Authentication, resources, claims and deletion |
| [004 — Restricted execution and customer model keys](004-execution.md) | MVP execution surface, capabilities and admission |
| [005 — One node and explicit durability](005-storage.md) | Stores, recovery boundaries and scaling ceiling |
| [006 — Evidence gates and release contracts](006-verification.md) | Performance, tests, compatibility and release |

These records decide only the proposed MVP. Future possibilities belong in the
[roadmap](../../ROADMAP.md), with new ADRs written when those decisions become necessary.
Cloud-account configuration, launch budgets and commercial policy are private Platform concerns.
