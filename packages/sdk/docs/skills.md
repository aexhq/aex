---
title: Skills
---

# Skills

A skill is a bundle of instructional or executable content (`SKILL.md` plus any
supporting files) that the agent can pull into context on demand. Skills are a
**first-class concept, separate from tools**: you pass them on the session's own
`skills` input (not `tools`), and the run gets a single `skills` meta-tool the
model uses to list and load them.

Build a skill with the `Skill.from*` factories. Each reads a bundle, lifts the
`name` and `description` from the `SKILL.md` YAML frontmatter (an explicit
`{ name }` overrides the frontmatter), canonically zips + hashes the bytes, and
returns a `Skill`:

- **Local directory:** `Skill.fromDir(rootDir, { name? })` reads a folder with
  `SKILL.md` at its root. With no `{ name }` and no frontmatter `name:`, the
  slugified directory basename is used (Bun/Node filesystem runtimes).
- **Signed URL:** `Skill.fromUrl(url, { name?, sha256?, timeoutMs?, fetch? })`
  fetches a zip archive with `SKILL.md` at the archive root (universal — needs a
  global `fetch`, or pass one).
- **In-memory:** `Skill.fromFiles({ name?, files })` from a path→bytes map,
  `Skill.fromContent(skillMd, { name? })` from a single `SKILL.md` string, or
  `Skill.fromBytes({ name?, zip })` from a pre-zipped bundle.

Names must match the skill-name pattern (`^[a-z0-9][a-z0-9_-]{0,127}$`), must not
contain `__` (reserved for MCP tools), and must not be the reserved names
`skills` / `skill`. The frontmatter `description` (max 2048 chars) is required.

```ts
import { Aex, Models, Skill } from "@aexhq/sdk";

const aex = new Aex({ apiKey });

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message,
  skills: [await Skill.fromDir("./skills/rules", { name: "rules" })],
  apiKeys: { anthropic: apiKey }
});
```

## By-name, mutable binding

A skill is bound to the workspace **by name**. `skill.upload(client)` UPSERTS the
workspace skill under its name; a re-upload under the same name changes what
every future run referencing that name sees. The wire ref is name-only —
`{ kind:"skill", name }` — with no `assetId` and no hash, so the idempotency hash
of two runs that reference the same skill name is identical even if the skill's
bytes changed between them.

Passing a **draft** `Skill` in `skills:` auto-upserts it on submit (the same
ergonomic as a draft `Tool` / `File`). To upsert explicitly and reuse the name
across many runs:

```ts
const rules = await Skill.fromDir("./skills/rules", { name: "rules" });
await rules.upload(aex);              // stage bytes + PUT /skills/rules
await aex.run({ model, message, skills: [rules], apiKeys });
```

`upload()` is idempotent on an instance (it caches the resolved name, so reuse
across submits skips the round-trip) and identical bytes are a server no-op.

### How a skill rides on the wire

Before the session lands, `openSession` / `run` walks the `skills` array and, for
each draft, upserts it by name:

1. `POST /assets/presign` checks for a dedup hit and, when needed, returns a
   signed upload URL.
2. The SDK PUTs bytes directly to object storage with the signed checksum headers.
3. `POST /assets/finalize` confirms the object exists.
4. `PUT /skills/{name}` binds that content hash to the mutable workspace skill
   name (identical `contentHash` ⇒ no-op).

The submission then carries only `submission.skills: [{ kind:"skill", name }]`.
The four prepare passes (tools, skills, agentsMd, files) upload with bounded
concurrency and run concurrently with each other.

## The `skills` meta-tool

When a run references at least one skill, the platform injects a single `skills`
meta-tool:

- `skills({ action: "list" })` returns each skill's `name` + `description`
  (cheap — do this first).
- `skills({ action: "load", name: "<skill>"})` reads that skill's full `SKILL.md`
  instructions into context.

A skill's supporting files are staged to disk under `/workspace/skills/<name>/`
from the first turn, so `load` pulls the instructions while `read_file` / `bash`
read the rest. The run resolves each referenced name to the workspace skill's
current bytes at submit time and snapshots them into durable run asset storage
(`runs/<runId>/assets/<hash>`); run-scoped copies are removed by run deletion or
retention cleanup.

## Workspace skill admin

`client.skills` is the metadata surface for the named registry:

- `client.skills.list()` — skill metadata (never bytes).
- `client.skills.get(name)` — one skill's metadata.
- `client.skills.delete(name)` — remove a workspace skill.

Skills that call external HTTP APIs should read credentials from
`environment.secrets` and use the normal client for that service. See
[Credentials](credentials.md) for the secret model.
