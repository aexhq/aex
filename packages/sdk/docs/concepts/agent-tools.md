---
title: Agent tools
description: The default builtin tools available inside managed runs.
icon: TerminalSquare
---

Managed runs inject a DX-first set of builtin tools to the agent by default. The
default set is every builtin tool EXCEPT `notebook_edit` (notebook editing is
opt-in). It includes:

- `bash`, `code_execution` — run shell commands / model-written snippets
- `read_file`, `write_file`, `edit_file` — file read/create/patch
- `grep`, `glob` — search file contents and paths
- `head`, `tail` — read bounded file slices
- `web_fetch`, `web_search` — fetch a URL / managed web search
- `todo_write` — maintain a todo list
- `subagent`, `subagent_result` — delegate to and read back from child runs
- `bash_output`, `bash_kill` — manage background bash jobs
- `wait`, `git` — bounded idle-yield and first-class git

## Toggling builtins

Set `includeBuiltinTools: false` to inject NO builtins — useful for a pure-MCP
or pure-custom run where every tool comes from `mcpServers` or `tools`.

`includeBuiltinTools` defaults to `true`.

## Cherry-picking builtins

The `tools` list accepts both custom tool bundles and BUILTIN tool references
(bare name strings, preferably `BuiltinTools.<name>`). Use a builtin reference
to add a tool the default set omits (notebook editing), or to pick a narrow
subset alongside `includeBuiltinTools: false`.

The final tool list is ordered: resolved builtin tools, then custom tools, then
MCP tools.

## Optional notebook support

`notebook_edit` edits Jupyter `.ipynb` cells as JSON. It is NOT in the default
builtin set; add it via `tools`:

```ts
import { BuiltinTools, Models } from "@aexhq/sdk";

await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Edit the analysis notebook.",
  tools: [BuiltinTools.notebook_edit],
  secrets: { apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! } }
});
```

Networking is open by default: the agent may reach any public host, subject to a
fixed SSRF deny-list. `web_fetch` and `web_search` reach the network over a
managed, SSRF-guarded path that is **not** governed by `environment.networking`,
so their hosts never need to be listed in a `limited` allowlist. Setting
`environment.networking.mode` to `limited` restricts only the agent's own
arbitrary egress (e.g. a `curl` in `bash`); the built-in web tools keep working.
See [Networking](../networking.md).

## Disable builtins

```ts
import { Models } from "@aexhq/sdk";

await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Use only the declared MCP tools.",
  mcpServers,
  includeBuiltinTools: false,
  secrets: { apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! } }
});
```
