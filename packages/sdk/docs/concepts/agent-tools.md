---
title: Agent tools
description: The default machine tools available inside managed runs.
icon: TerminalSquare
---

Managed runs expose a fixed set of machine tools to the agent. They are on by
default. Omit `builtins` or pass any non-empty list to keep them; pass
`builtins: []` for a pure-MCP run with no machine tools. MCP-derived tools and
subagent tools are separate surfaces.

## Filesystem and shell

- `bash` runs shell commands in the run container.
- `read_file`, `write_file`, and `edit_file` read, write, or patch files.
- `grep` and `glob` search file contents and paths.
- `head` and `tail` read bounded file slices.

## Background commands

`bash` can run detached with `run_in_background: true`. The agent can then poll
with `bash_output` or stop the job with `bash_kill`. This is useful for dev
servers, long builds, and log tails.

## Planning, notebooks, and web

- `todo_write` records the agent's current task list.
- `notebook_edit` edits Jupyter `.ipynb` cells as JSON.
- `web_fetch` fetches a URL and returns readable text.
- `web_search` performs managed web search without requiring a caller-supplied
  search key.

Networking policy still applies. Under limited networking, fetched hosts and
the managed search host must be allowed by the run's networking configuration.

## Disable machine tools

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
