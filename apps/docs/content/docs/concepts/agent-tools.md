---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/concepts/agent-tools.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Agent tools
description: Select builtin capabilities and attach published custom tools.
icon: TerminalSquare
---

`builtinTools` controls platform-provided capabilities:

- `"default"` enables the standard set and is the default.
- `"none"` disables all builtin tools.
- An array such as `[BuiltinTools.bash, BuiltinTools.read_file]` selects an
  explicit subset.

Custom tools are workspace resources. Build and publish them, then attach the
returned ref under `assets.tools`.

```ts
import { BuiltinTools, Tool } from "@aexhq/sdk";

const draft = await Tool.fromFiles({
  name: "lookup_metric",
  description: "Looks up one metric.",
  inputSchema: {
    type: "object",
    properties: { name: { type: "string" } },
    required: ["name"]
  },
  entry: "index.js",
  files: {
    "index.js": "export default async ({ input }) => ({ content: [{ type: 'text', text: input.name }] });"
  }
});
const lookup = await aex.workspace.tools.publish(draft);

await aex.start({
  model,
  message: "Look up revenue and save the result.",
  builtinTools: [BuiltinTools.write_file],
  assets: { tools: [lookup] }
});
```

Tool entries must be `.js`, `.mjs`, or `.cjs` modules present in the bundle.
The SDK validates the manifest before any network request.

Use `mcpServers` for remote MCP tools. The final runtime tool set consists of
the selected builtins, pinned custom tools, and declared MCP tools.
