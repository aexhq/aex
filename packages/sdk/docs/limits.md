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
| Workspace storage | 500 GB per workspace for captured files and workspace artifacts. |

## Product Boundaries

| Area | Boundary |
| --- | --- |
| Runtime | New submissions run on a managed runtime. `runtime.kind` selects `container` (the default), `spot_container`, or `lambda`; `runtime.size` selects a managed machine-size preset (`Sizes.*`). Both fields are optional. |
| Provider policy | Provider retention, training exclusion, HIPAA/BAA, data residency, abuse policy, and pricing belong to the selected provider account, endpoint, and contract. |
| Secrets | Provider keys, MCP credentials, and environment secrets are caller-owned. aex excludes secret values from idempotency and uses the explicit secret surfaces described in [Secrets](secrets.md). |
| MCP servers | Remote MCP servers are customer-trusted systems. aex validates declarations and routes credentials; it does not make an untrusted MCP server safe. |
| Files | Captured files, events, and metadata are stored under the session record and downloaded through auth-gated routes. SessionFile content is customer content. |
| Human review | Sessions execute after submission. Cancellation is available, but aex does not pause a session for platform-mediated approval or interactive clarification. |
| Sessions | The durable product primitive is the session record. Sessions can be resumed by id and auto-suspend after the configured idle window; persistent named agent profiles and saved agent definitions are out of scope. |
| Hosting | The SDK and CLI connect to the hosted aex API. `baseUrl` may also target the localhost development stack; self-hosting is not supported. |
| Provider cost | BYOK provider-token charges accrue to the selected provider account. Finished results report the cost and provider usage recorded for that run. |

## Provider Policy Links

These links are starting points for provider-owned policy areas; they do not
create aex guarantees.

- Anthropic API data retention policy: <https://platform.claude.com/docs/en/manage-claude/api-and-data-retention>
- OpenAI API data controls: <https://platform.openai.com/docs/guides/your-data>
- Mistral privacy and API data handling: <https://docs.mistral.ai/admin/security-access/privacy>
- Gemini API data handling: <https://ai.google.dev/gemini-api/docs/logs-policy>

## Unsupported Claims

Do not describe aex as providing self-hosting, provider-wide retention,
HIPAA/BAA or data-residency guarantees, a general-purpose sandbox for every
downstream service, human-in-the-loop approval checkpoints, or persistent agent
identity.
