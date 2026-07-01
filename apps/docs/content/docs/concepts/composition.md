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
| Executable or instructional bundles | `Skill.fromPath`, `Skill.fromFiles`, `Skill.fromUrl`, `Skill.fromCatalog` |
| Agent instructions | `AgentsMd.fromPath`, `AgentsMd.fromContent` |
| Reference files and folders | `File.fromPath`, `File.fromBytes` |
| Remote tools | `McpServer.remote`, `McpServer.fromId` |
| Credentialed HTTP APIs | `ProxyEndpoint.none`, `bearer`, `basic`, `header`, `query` |
| Non-secret runtime settings | `environment.variables`, `environment.packages`, `environment.networking` |

```ts
import { AgentsMd, File, McpServer, Models, ProxyEndpoint, Skill } from "@aexhq/sdk";

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Use the attached docs and tools to produce a report.",
  agentsMd: [AgentsMd.fromContent("Follow the repo conventions.")],
  files: [await File.fromPath("./input")],
  skills: [await Skill.fromPath("./skills/report-writer", { name: "report-writer" })],
  mcpServers: [McpServer.remote({ name: "github", url: "https://example.com/mcp" })],
  proxyEndpoints: [
    ProxyEndpoint.bearer({
      name: "internal-api",
      baseUrl: "https://api.example.com",
      token: process.env.INTERNAL_API_TOKEN!,
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1/"]
    })
  ],
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Secrets stay out of reusable configs. Provider keys go in the top-level `apiKeys`
map; MCP auth rides on each `McpServer` instance and proxy auth on each
`ProxyEndpoint` instance — the SDK splits them into the vaulted secrets channel
server-side, so they never live in a shareable config object.
