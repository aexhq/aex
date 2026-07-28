---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/concepts/composition.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Composition
description: Publish reusable workspace resources and pin them to sessions.
icon: Blocks
---

Files, skills, custom tools, and instructions follow one lifecycle:

1. Build a local draft with `File`, `Skill`, `Tool`, or `Instructions`.
2. Publish it through `aex.workspace.<kind>.publish(...)`.
3. Pass the returned immutable, version-pinned ref under `assets`.

```ts
import { Aex, File, Instructions, Skill } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const [input, rules, reportWriter] = await Promise.all([
  File.fromPath("./input").then((draft) => aex.workspace.files.publish(draft)),
  Instructions.fromContent("Follow the repository conventions.", { name: "repo-rules" })
    .then((draft) => aex.workspace.instructions.publish(draft)),
  Skill.fromDir("./skills/report-writer", { name: "report-writer" })
    .then((draft) => aex.workspace.skills.publish(draft))
]);

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Use the attached material to produce a report.",
  assets: {
    files: [input],
    instructions: [rules],
    skills: [reportWriter]
  },
});
```

Published refs contain `resourceId`, `version`, `assetId`, and `contentHash`.
Changing a logical resource creates a new version; an existing session remains
pinned to the exact bytes it received.

An `Instructions` name is the persisted workspace resource name supplied by
the caller. It is preserved exactly—never trimmed, lowercased, or otherwise
normalized—and must be 1–128 ASCII characters: an alphanumeric first character,
then alphanumeric characters, `.`, `_`, or `-`. The `__` sequence is reserved.
This direct-name contract is deliberately different from `File`, which accepts
a filename-shaped input and derives a shorter lowercase storage slug while
preserving the real mounted filename.

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
