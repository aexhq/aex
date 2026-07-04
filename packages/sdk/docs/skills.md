---
title: Skills
---

# Skills

A skill is a bundle of instructional or executable content (`SKILL.md` plus any
supporting files) that the agent can pull into context on demand. In the SDK a
skill is expressed as a per-skill **load-tool**: it rides in the session's
`tools` array next to builtin tool names and custom `Tool` bundles, and the model
loads it by calling it.

Build a skill-tool with the `Tools.fromSkill*` factories. Each factory reads a
skill bundle, lifts the tool `name` and `description` from the `SKILL.md` YAML
frontmatter (an explicit `{ name }` overrides the frontmatter), canonically
zips + hashes the bytes, and returns a `SkillTool`:

- **Local directory:** `Tools.fromSkillDir(rootDir, { name? })` reads a folder
  that has `SKILL.md` at its root (Bun/Node filesystem runtimes).
- **Signed URL:** `Tools.fromSkillUrl(url, { name?, sha256?, timeoutMs?, fetch? })`
  fetches a zip archive with `SKILL.md` at the archive root (universal — needs a
  global `fetch`, or pass one).

```ts
import { Aex, Models, Tools } from "@aexhq/sdk";

const aex = new Aex({ apiKey });

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message,
  tools: [await Tools.fromSkillDir("./skills/rules", { name: "rules" })],
  apiKeys: { anthropic: apiKey }
});
```

`Tools.fromSkillDir("./skills/rules", …)` resolves relative to the process CWD,
so run the script from the directory that *contains* `skills/`. The `SKILL.md`
frontmatter must supply a `description` (max 2048 chars) and, unless you pass
`{ name }`, a `name`. Names must match the tool-name pattern and must not contain
`__` (that separator is reserved for MCP tools).

## How a skill-tool rides on the wire

Before the session lands, `openSession` / `run` walks the `tools` array and
uploads each draft skill-tool's bundle through the asset upload flow:

1. `POST /assets/presign` checks for a dedup hit and, when needed, returns a
   signed upload URL.
2. The SDK PUTs bytes directly to object storage with the signed checksum headers.
3. `POST /assets/finalize` confirms the object exists.

The wire ref then becomes a `{ kind:"skill", assetId, name, description }` entry
inside `submission.tools`. Identical bytes dedup by content hash, so re-submitting
the same bundle is a no-op upload; a `SkillTool` instance also caches its resolved
asset id, so reusing the same instance across sessions skips the re-upload. A URL
is an ingestion source, not a persistent reference — the SDK snapshots the fetched
bytes into the asset store, and the hosted platform never fetches the
caller-controlled URL.

## Loading and materialization

The skill-tool's `name` and `description` are what the agent sees in its tool
list. At run time the model calls the **no-arg load-tool** to pull the skill's
`SKILL.md` body into context — the description tells the agent when that is worth
doing.

Independently of whether the model calls the load-tool, the platform copies the
referenced skill asset into durable run asset storage
(`runs/<runId>/assets/<hash>`) and the runner **eagerly stages** the bundle's
files into the workspace under `/workspace/skills/<name>/`. So the `SKILL.md` body
and every supporting file are on disk from the first turn; the load-tool call is
how that body enters the model's context, not how the files get written.

Skills that call external HTTP APIs should read credentials from
`environment.secrets` and use the normal client for that service. See
[Credentials](credentials.md) for the secret model.

Run-scoped asset copies are part of the run record and are removed by run deletion
or retention cleanup.
