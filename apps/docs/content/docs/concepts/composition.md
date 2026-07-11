---
title: Composition
description: Publish reusable workspace resources and pin them to sessions.
icon: Blocks
---

Files, skills, custom tools, and instructions follow one lifecycle:

1. Build a local draft with `File`, `Skill`, `Tool`, or `Instructions`.
2. Publish it through `aex.workspace.<kind>.publish(...)`.
3. Pass the returned immutable, version-pinned ref under `assets`.

```ts
import { Aex, File, Instructions, Models, Skill } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const [input, rules, reportWriter] = await Promise.all([
  File.fromPath("./input").then((draft) => aex.workspace.files.publish(draft)),
  Instructions.fromContent("Follow the repository conventions.", { name: "repo-rules" })
    .then((draft) => aex.workspace.instructions.publish(draft)),
  Skill.fromDir("./skills/report-writer", { name: "report-writer" })
    .then((draft) => aex.workspace.skills.publish(draft))
]);

await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Use the attached material to produce a report.",
  assets: {
    files: [input],
    instructions: [rules],
    skills: [reportWriter]
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Published refs contain `resourceId`, `version`, `assetId`, and `contentHash`.
Changing a logical resource creates a new version; an existing session remains
pinned to the exact bytes it received.

Builtin capabilities stay in `builtinTools`, and remote MCP servers stay in
`mcpServers`. Neither is mixed into `assets.tools`.

Workspace lists are cursor paged:

```ts
const page = await aex.workspace.files.list({ limit: 100 });
const next = page.nextCursor
  ? await aex.workspace.files.list({ cursor: page.nextCursor, limit: 100 })
  : undefined;
```

The default and maximum page size are both 100.
