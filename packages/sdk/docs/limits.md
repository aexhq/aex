---
title: Limits
---

# Limits

aex runs autonomous agents on the hosted managed runtime. The SDK and CLI submit
runs, stream events, capture outputs, and expose auth-gated reads and downloads.

For what the product supports, see [Features](https://aex.dev/docs/features/).
For the current provider/model set, see the generated
[provider/runtime capability matrix](provider-runtime-capabilities.md).

## Current Defaults

| Area | Default |
| --- | --- |
| Workspace storage | 50 GiB per workspace for captured outputs and workspace artifacts. aex-maintainer admin workspaces may be unlimited for internal dogfooding; this is not a customer entitlement. |
| Proxy request body | 10 MiB per proxy endpoint unless the endpoint declares a different `maxRequestBytes`. |
| Proxy timeout | 5 minutes per proxy endpoint unless the endpoint declares a different `timeoutMs`. |
| Proxy telemetry | Proxy calls emit report-only usage telemetry for call count, failed calls, request bytes, response bytes when known, and duration. Public proxy pricing is not shipped unless documented later. |

## Product Boundaries

| Area | Boundary |
| --- | --- |
| Runtime | New submissions run on the managed runtime. `runtime: "native"` is rejected. |
| Provider policy | Provider retention, training exclusion, HIPAA/BAA, data residency, abuse policy, and pricing belong to the selected provider account, endpoint, and contract. |
| Secrets | Provider keys, MCP credentials, proxy auth, and env secrets are caller-owned. aex excludes secret values from idempotency and uses the explicit secret surfaces described in [Secrets](secrets.md). |
| MCP servers | Remote MCP servers are customer-trusted systems. aex validates declarations and routes credentials; it does not make an untrusted MCP server safe. |
| Proxy endpoints | The proxy enforces declared host/path/method/auth policy for calls routed through it. Upstream side effects and data handling remain with the upstream service and customer. |
| Outputs | Captured outputs, events, and metadata are stored under the run record and downloaded through auth-gated routes. Output content is customer content. |
| Human review | Runs execute after submission. Cancellation is available, but aex does not pause a run for platform-mediated approval or interactive clarification. |
| Agent identity | The durable product primitive is the run record. Persistent agent profiles, stateful memory, reusable sessions, and saved agent definitions are out of scope. |
| Deployment | The supported product is the hosted aex service plus the SDK and CLI. Alternate `baseUrl` values are for local, staging, or hosted aex API planes, not a self-host product promise. |
| Cost | BYOK provider-token charges accrue to the customer's provider account. aex records report-only telemetry for runtime, storage, and proxy usage; free trials, billing-grade invoices, and public pricing documents are not shipped unless documented later. |

## Provider Policy Links

These links are starting points for provider-owned policy areas; they do not
create aex guarantees.

- Anthropic API data retention policy: <https://platform.claude.com/docs/en/manage-claude/api-and-data-retention>
- OpenAI API data controls: <https://platform.openai.com/docs/guides/your-data>
- Mistral privacy and API data handling: <https://docs.mistral.ai/admin/security-access/privacy>
- Gemini API data handling: <https://ai.google.dev/gemini-api/docs/logs-policy>

## Unsupported Claims

Do not describe aex as providing true self-host or customer-cloud deployment
support, provider-wide retention, HIPAA/BAA or data-residency guarantees, free
trials, a general-purpose sandbox for every downstream service, human-in-the-loop
approval checkpoints, or persistent agent identity.
