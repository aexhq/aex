---
title: Skills
---

# Skills

Skill inputs accepted by the platform:

- provider-managed Anthropic prebuilt Agent Skills (`pdf`, `xlsx`,
  `docx`, `pptx` — see the
  [Anthropic Agent Skills overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
  for the live catalog). Note that Anthropic web search is a Messages
  API *tool* (`{ type: "web_search_…" }`), not an Agent Skill, and is
  not currently exposed through `submitRun`;
- existing custom provider skill IDs;
- workspace skill bundles (persistent, referenced by `skl_*` id);
- inline-supplied bundles passed directly at `submitRun` — these
  persist on aex as workspace skills with auto-suffixed names,
  one row per submission (see "Inline supply" below).

**How skills reach the agent:** every skill bundle is materialized into
the run's workspace under `skills/<name>/` before the first agent turn.
A bundle's `SKILL.md` is composed into the agent's instructions, so the
agent is told the skill exists and what it does without the model having
to discover it. Bundles without `SKILL.md` are still mounted as files at
`skills/<name>/`, but nothing prompts the agent to read them — reference
them explicitly from the prompt or your `AGENTS.md`.

The platform also mounts the `aex` CLI and a per-run manifest into the
workspace on **every** run. Skills invoke the managed HTTP proxy via the
mounted CLI (`aex proxy …`) — see `credentials.md` for the policy/auth
model.

## Inline supply at `submitRun`

`Skill.fromFiles({name, files})` and `Skill.fromPath(rootDir, {name})`
build an **unstaged** `Skill`. The instance carries the canonicalised
zip bytes and the `sha256:<hex>` content hash:

```ts
import { AgentExecutor, Skill } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken });

await aex.submitRun({
  model, prompt,
  skills: [await Skill.fromFiles({ name: "rules", files })],
  secrets: { anthropic: { apiKey } }
});
```

`aex.submitRun` walks the `skills` array, sends a multipart body
alongside the JSON submission, and materializes the bytes to
content-addressable, workspace-scoped asset storage before the run lands.
The BFF re-canonicalises the bundle, verifies the advisory hash, and
persists it as a workspace skill — but with an **auto-suffixed name**
(`rules-x8q7lk2`) so repeated supplies of the same logical skill across
many runs produce distinct `skl_*` rows. This is by design: a
submitted skill is a **per-run artifact**.

`contentHash` is `sha256:<hex>` of the canonical bundle zip (the SDK
normalises file order, mtime, and permissions before hashing).
Identical inputs always produce the same hash, so the same bytes are a
no-op upload (content-addressable dedup) on subsequent runs. There is no
separate workspace pre-upload step.

### What auto-suffix looks like

Submit `Skill.fromFiles({ name: "rules", files })` three times across
three runs and the dashboard shows three distinct skills:

```
rules-x8q7lk2
rules-mp2vqa1
rules-az3lkmt
```

Same prefix, different suffix. Each is a real workspace skill — you
can list, get, download, and delete it through the regular
`aex.skills.*` verbs.

### Deletion semantics

Soft-deleting a skill (`aex.skills.delete(skl_id)` or the
dashboard's Delete button) marks the row tombstoned. Existing runs
that pinned a snapshot of the skill keep working — they read from
`run_skill_snapshots` which preserves the name, hash, size, and
manifest. The run detail view shows a "deleted" badge for those
orphan references; the download endpoint returns
**HTTP 410 Gone** with `{ error: { code: "skill_deleted", … } }`
when the caller tries to fetch the bytes of a deleted skill.

New `submitRun` calls referencing a soft-deleted `skl_*` id are
rejected before insertion.

## Fetch from a signed URL (`Skill.fromUrl`)

When your app runs in the cloud with limited storage, you may not want to
bundle skill bytes with it. Host the skill yourself as a **zip archive**
(with `SKILL.md` at the archive root) and hand the SDK a temporary signed
URL — e.g. an S3 presigned URL:

```ts
import { AgentExecutor, Skill } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken });

await aex.submitRun({
  model, prompt,
  skills: [
    await Skill.fromUrl(signedUrl, { name: "rules", sha256: "sha256:<hex>" })
  ],
  secrets: { anthropic: { apiKey } }
});
```

`Skill.fromUrl` fetches the archive **in the SDK process** — the URL is
caller-controlled, so there is no server-side fetch — optionally verifies the
download against `sha256`, unzips it, and reduces it to the same files map as
`Skill.fromFiles`. A URL-sourced skill and the identical local skill therefore
produce the **same canonical asset** and dedup against each other.

- The archive must contain `SKILL.md` at its root, or inside a single
  top-level folder, which is stripped automatically. Anything else is
  rejected with an error listing the archive's actual top-level entries.
- The signed URL only needs to be valid **for this call**. `aex.submitRun`
  snapshots the bytes into the run immediately, so the URL can expire
  afterwards with no effect on the run.
- `sha256` is an optional source-integrity check on the downloaded archive
  (distinct from the canonical bundle hash); a mismatch fails fast before the
  unzip. Signed-URL query strings are never echoed in error messages.
- The same upload caps apply (10 MB compressed / 50 MB decompressed /
  1000 files); `Skill.fromUrl` materialises the whole bundle, it is not a
  streaming mount.

`Skill.fromUrl` is universal (Node 18+ / browser): it uses the global `fetch`,
or pass one via `{ fetch }`.
