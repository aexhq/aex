---
title: Composition
description: The primitives used to assemble each run.
icon: Blocks
---

aex composes an agent from explicit per-session inputs. The SDK materializes local
bytes before the session lands and the platform mounts them into the managed
runtime before the first agent turn.

| Need | Primitive |
| --- | --- |
| Executable or instructional skill bundles (load-tools) | `Tools.fromSkillDir`, `Tools.fromSkillUrl` |
| Agent instructions | `AgentsMd.fromPath`, `AgentsMd.fromContent` |
| Reference files and folders | `File.fromPath`, `File.fromBytes` |
| Remote tools | `McpServer.remote`, `McpServer.fromId` |
| Non-secret runtime settings | `environment.variables`, `environment.packages`, `environment.networking` |
| Runtime secrets for your code | `Secret.value`, `Secret.ref`, `environment.secrets` |

```ts
import { AgentsMd, File, McpServer, Models, Secret, Tools } from "@aexhq/sdk";

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Use the attached docs and tools to produce a report.",
  agentsMd: [AgentsMd.fromContent("Follow the repo conventions.")],
  files: [await File.fromPath("./input")],
  tools: [await Tools.fromSkillDir("./skills/report-writer", { name: "report-writer" })],
  mcpServers: [McpServer.remote({ name: "github", url: "https://example.com/mcp" })],
  environment: {
    secrets: { INTERNAL_API_TOKEN: Secret.value(process.env.INTERNAL_API_TOKEN!) },
    networking: { mode: "limited", allowedHosts: ["api.example.com"] }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Secrets stay out of reusable configs. Provider keys go in the top-level `apiKeys`
map; reusable or per-run values for your own code go in `environment.secrets`.
Your code then makes normal HTTP calls with the standard client for that service.
