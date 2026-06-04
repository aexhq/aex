---
title: MCP
---

# MCP

MCP support is remote HTTPS/SSE only. Stdio MCP servers are rejected because aex is a remote session dispatcher, not a local process supervisor.

Rules:

- MCP servers are declared in the run request config (`mcpServers`).
- Runtime HITL is disabled.
- Tool policy must be configured before session start.
- Enabled MCP tools use `always_allow` provider permissions.
- `always_ask` is not used by aex MVP.
- Bearer/OAuth-style auth is passed in the per-run `secrets.mcpServers` bundle.

Use allowlists for sensitive servers whenever possible.

## Large-payload responses

aex is a session dispatcher, not an MCP runtime. We intentionally do
**not** interpose on the transport between Claude and an upstream MCP
server, so we cannot elide MCP responses or write them to the session
filesystem on the user's behalf. Anything an MCP tool returns lands
directly in the model's context.

For ingestion-style tools that return large JSON blobs (search results,
catalogue dumps, bulk reads), use the **CLI-as-skill + managed proxy**
pattern instead of MCP:

1. Package the upstream as a `Skill` — a CLI binary the agent invokes
   with its bash tool.
2. Route every upstream HTTPS call through a per-run `ProxyEndpoint`
   (audit, byte caps, budget enforcement).
3. Have the CLI write the full payload under one of the directories you
   passed to `outputDirs`. Return only a small handle (path, item count,
   summary) to the model.

The agent sees the handle in context; the bytes ride out through
`download()` as a normal captured output.

If you genuinely want everything in context (small responses, code
search, etc.), use MCP. If the payload would blow your context budget,
the CLI-as-skill pattern is the supported answer — there is no platform
flag to elide MCP responses.
