---
title: Agent tools
description: The default machine tools available inside managed runs.
icon: TerminalSquare
---

Managed runs expose a DX-first default builtin set to the agent. Omit
`builtins` for the recommended defaults:

- `web_search`
- `web_fetch`
- `read`
- `edit`
- `glob`
- `grep`
- `head`
- `tail`
- `bash`

Pass `builtins: []` for a pure-MCP run with no builtins. Pass a custom list to
narrow or extend the surface, for example adding the optional `notebook`
builtin. MCP-derived tools and subagent tools are separate surfaces.

## Default filesystem and web tools

- `read` and `edit` expose file read/create/patch tools.
- `grep` and `glob` search file contents and paths.
- `head` and `tail` read bounded file slices.
- `bash` runs shell commands in the run sandbox.
- `web_fetch` fetches a URL and returns readable text.
- `web_search` performs managed web search without requiring a caller-supplied
  search key.

## Optional notebook support

`notebook` edits Jupyter `.ipynb` cells as JSON. It is accepted by the
contract, but it is not in the default builtin list.

Networking is open by default. If you explicitly set
`environment.networking.mode` to `limited`, fetched hosts and the managed search
host must be allowed by the run's networking configuration.

## Disable builtins

```ts
import { Models } from "@aexhq/sdk";

await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Use only the declared MCP tools.",
  mcpServers,
  builtins: [],
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```
