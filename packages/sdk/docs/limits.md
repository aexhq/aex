---
title: Limits
---

# Limits

aex sessions autonomous agents on the hosted managed runtime. The SDK opens durable
sessions, sends turns, streams events, captures files, and exposes auth-gated
reads and downloads.

For what the product supports, see [Features](https://aex.dev/docs/features/).
For the current provider/model set, see the generated
[provider/runtime capability matrix](provider-runtime-capabilities.md).

## Current Defaults

| Area | Default |
| --- | --- |
| Workspace storage | 500 GB per workspace for captured files and workspace artifacts. aex-maintainer admin workspaces may be unlimited for internal dogfooding; this is not a customer entitlement. |

## Product Boundaries

| Area | Boundary |
| --- | --- |
| Runtime | New submissions run on the managed runtime. The `runtime` option selects a managed machine-size preset (`Sizes.*`); there is no alternative runtime backend. |
| Provider policy | Provider retention, training exclusion, HIPAA/BAA, data residency, abuse policy, and pricing belong to the selected provider account, endpoint, and contract. |
| Secrets | Provider keys, MCP credentials, and env secrets are caller-owned. aex excludes secret values from idempotency and uses the explicit secret surfaces described in [Secrets](secrets.md). |
| MCP servers | Remote MCP servers are customer-trusted systems. aex validates declarations and routes credentials; it does not make an untrusted MCP server safe. |
| Files | Captured files, events, and metadata are stored under the session record and downloaded through auth-gated routes. SessionFile content is customer content. |
| Human review | Sessions execute after submission. Cancellation is available, but aex does not pause a session for platform-mediated approval or interactive clarification. |
| Sessions | The durable product primitive is the session/session record. Sessions can be resumed by id and auto-suspend after the configured idle window; persistent named agent profiles and saved agent definitions are out of scope. |
| Deployment | The supported product is the hosted aex service plus the SDK and CLI. Alternate `baseUrl` values are for local, staging, or hosted aex API planes, not a self-host product promise. |
| Cost | BYOK provider-token charges accrue to the customer's provider account. aex records report-only telemetry for runtime and storage usage; free trials, billing-grade invoices, and public pricing documents are not shipped unless documented later. |

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
