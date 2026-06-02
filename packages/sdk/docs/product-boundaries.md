---
title: Product capabilities and boundaries
---

# Product capabilities and boundaries

antpath is the serverless control plane for autonomous agent sessions. It accepts a complete run request, dispatches it to a provider-native runtime or Goose Managed, records ordered events and logs, captures outputs, and exposes auth-gated reads and downloads.

antpath is not a custom agent loop, a general-purpose sandbox, an interactive approval system, or a provider compliance layer. True self-host and customer-cloud deployment modes are not supported today.

Start with the generated [provider/runtime capability matrix](provider-runtime-capabilities.md) for supported providers, runtime routing, native feature parity, and evidence pointers.

## Owned by antpath today

- Run submission, idempotency, status, cancellation, reads, downloads, and workspace auth.
- Runtime dispatch across Anthropic Native and Goose Managed, with unsupported runtime/provider combinations rejected at submission.
- Ordered event/log capture through the per-run coordinator and durable archive.
- Output capture into the run record, subject to runtime behavior and storage limits.
- BYOK provider-key custody for a single run, using the top-level `secrets` carrier and terminal cleanup/revocation attempts.
- Named proxy endpoint policy, auth injection, redaction, call budgets, and audit metadata on the antpath-owned proxy path.
- Default cleanup attempts for tracked antpath/provider session resources, with provider retention respected when `cleanup.session: "retain"` is requested.

## Boundary matrix

| Area | antpath-owned behavior | Inherited or customer-owned behavior |
| --- | --- | --- |
| Provider and model policy | antpath validates the selected provider, injects the run-scoped BYOK credential, and records public-safe runtime events. | Provider retention, training exclusion, zero-retention, HIPAA/BAA, data residency, abuse policy, and pricing are properties of the selected provider account, endpoint, and contract. |
| Runtime isolation | Goose Managed runs in an isolated managed runtime. Anthropic Native runs in Anthropic's hosted runtime. antpath tracks resources and runs cleanup attempts. | Runtime isolation guarantees belong to the managed runtime provider for Goose and to the provider for native runtimes. antpath does not turn every runtime into the same sandbox. |
| Secrets | Provider keys, MCP credentials, and proxy auth values are supplied inline per run, held in run-scoped custody, excluded from idempotency, and targeted for cleanup/revocation at terminal. | Customers choose and rotate their provider keys and MCP/proxy credentials. Provider-side credentials, sessions, and data may have their own retention rules. |
| MCP servers | antpath accepts remote HTTP/SSE MCP servers, validates their declaration, attaches run-scoped credentials, and records access metadata on the antpath-controlled edge. | MCP servers are customer-trusted remote systems. antpath does not sandbox their downstream behavior or make an untrusted MCP server safe. |
| Proxy endpoints | The named endpoint proxy enforces declared host/path/method/auth policy and response caps for calls routed through it. | The upstream service's own auth, data handling, side effects, and compliance posture remain with the upstream service and customer. |
| Outputs and run record | Captured outputs, events, logs, and metadata are stored under the run record and downloaded through auth-gated routes. | Output content is customer content. Storage, deletion, and retention follow the run policy and infrastructure behavior; deletion-proof custody manifests are roadmap work until shipped. |
| Human review | Runs execute full-auto after submission. Cancellation is available as an abort control. | Required input, approval, and planning happen before submission or after inspecting the completed run record. antpath does not pause a run for platform-mediated human approval or interactive clarification. |
| Agent identity and memory | The durable product primitive is the run record, addressed by run id. | Persistent agent identity, agent profiles, stateful memory, reusable provider sessions, and saved-definition products are out of scope. |
| Deployment model | The repository contains source for the SDK, CLI, contracts, conformance helpers, user-test harness, and docs. | Hosted platform implementation, deployment workflows, billing/rate logic, and provider/substrate adapters are private. True self-host and customer-cloud deployments are not supported product modes today. Alternate `baseUrl` values are for local, staging, private, or hosted antpath API planes, not a self-host promise. |
| Cost | BYOK provider-token charges accrue to the customer's provider account. antpath can expose run/runtime/output metadata that helps operators reason about usage. | Paid managed-key mode, free trials, billing-grade cost telemetry, public rate cards, margins, and reconciliation are not shipped in the public product unless explicitly documented later. |

## Provider and infrastructure policy links

Use these links as starting points for the policy areas antpath does not own:

- Anthropic API data retention and Managed Agents policy: <https://platform.claude.com/docs/en/manage-claude/api-and-data-retention>
- OpenAI API data controls: <https://platform.openai.com/docs/guides/your-data>
- Mistral privacy and API data handling: <https://docs.mistral.ai/admin/security-access/privacy>
- Gemini API data handling: <https://ai.google.dev/gemini-api/docs/logs-policy>
These links do not create antpath guarantees. They identify the provider whose current terms and product behavior must be reviewed for a given workload.

## Non-goals and unsupported claims

Do not describe antpath as providing:

- true self-host or customer-cloud deployment support is not supported today;
- antpath does not provide zero retention, HIPAA, BAA, or data-residency guarantees across providers;
- antpath does not provide a free trial or low-cost managed-key mode;
- a general-purpose sandbox for every runtime and downstream MCP service;
- human-in-the-loop approval checkpoints, ask-the-user loops, or interactive resume;
- persistent agent identity, agent profiles, stateful memory, reusable sessions, or saved agent definitions.
