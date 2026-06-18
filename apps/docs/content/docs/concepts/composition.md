---
title: Composition
description: The primitives used to assemble each run.
icon: Blocks
---

aex composes an agent from explicit per-run inputs. The SDK materializes local
bytes before submission and the platform mounts them into the managed runtime
before the first agent turn.

| Need | Primitive |
| --- | --- |
| Executable or instructional bundles | `Skill.fromPath`, `Skill.fromFiles`, `Skill.fromUrl`, `Skill.fromCatalog` |
| Agent instructions | `AgentsMd.fromPath`, `AgentsMd.fromContent` |
| Reference files and folders | `File.fromPath`, `File.fromBytes` |
| Remote tools | `McpServer.remote`, `McpServer.fromId` |
| Credentialed HTTP APIs | `ProxyEndpoint.none`, `bearer`, `basic`, `header`, `query` |
| Non-secret runtime settings | `environment.envVars`, `environment.packages`, `environment.networking` |

```ts
import { AgentsMd, File, McpServer, Models, ProxyEndpoint, Skill } from "@aexhq/sdk";

await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Use the attached docs and tools to produce a report.",
  agentsMd: [AgentsMd.fromContent("Follow the repo conventions.")],
  files: [await File.fromPath("./input")],
  skills: [await Skill.fromPath("./skills/report-writer", { name: "report-writer" })],
  mcpServers: [McpServer.remote({ name: "github", url: "https://example.com/mcp" })],
  proxyEndpoints: [
    ProxyEndpoint.bearer({
      name: "internal-api",
      baseUrl: "https://api.example.com",
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1/"]
    })
  ],
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

Secrets stay out of reusable configs. Put provider keys, MCP auth, and proxy
auth in the `secrets` bundle on the concrete submit call.
