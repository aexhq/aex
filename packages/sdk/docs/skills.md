---
title: Skills
---

# Skills

A skill is executable or instructional content that is mounted into a run before
the first agent turn. Every accepted skill ends up as a storage-neutral
`kind:"asset"` reference in the run submission, and the hosted platform snapshots
that asset into the run's R2 prefix before dispatch.

There are three sources for skill bytes:

- **Inline/local draft:** `Skill.fromFiles(...)`, `Skill.fromPath(...)`, or
  `Skill.fromUrl(...)` builds a draft in the SDK process. `submitRun` uploads
  it before posting `/runs`.
- **Pre-uploaded workspace asset:** call `await draft.upload(aex)` and reuse the
  returned materialized `Skill`, or pass an existing `kind:"asset"` ref from a
  config file.
- **Workspace skill catalog:** upload with `aex skills upload` or the dashboard,
  then pass the returned record to `Skill.fromCatalog(record)`.

All three sources normalize to the same content-addressed asset. Identical bytes
dedup by hash, so repeated submissions of the same bundle are no-op uploads.
There is no per-run auto-suffixed `skl_*` row for inline skills.

Provider-hosted skill refs (`kind:"provider"`, e.g. Anthropic prebuilt Agent
Skills or custom provider skill IDs) are not supported on the managed runtime.
Every new submission dispatches to managed, so a `kind:"provider"` ref is
rejected at submission time with `feature_runtime_mismatch`. Supply the bytes as
an aex asset instead.

## Materialization

For each run, the platform copies referenced skill assets into that run's R2
directory (`runs/<runId>/assets/<hash>`) and the runner downloads them into the
workspace under `skills/<name>/`.

A bundle's `SKILL.md` is composed into the agent's instructions, so the agent is
told the skill exists and what it does without needing to discover it. Bundles
without `SKILL.md` are still mounted as files at `skills/<name>/`, but nothing
prompts the agent to read them; reference them explicitly from the prompt or
your `AGENTS.md`.

The platform also mounts the `aex` CLI and a per-run manifest into every run.
Skills call managed HTTP proxy endpoints through the mounted CLI
(`aex proxy ...`); see `credentials.md` for the policy and auth model.

Run-scoped R2 copies are part of the run record and are removed by run deletion
or retention cleanup. Catalog assets are separate workspace records: deleting a
catalog skill hard-deletes its metadata and removes the shared R2 object only
when no other catalog row still references those bytes. Existing run snapshots
keep their run-scoped copy.

## Inline And Local Drafts

`Skill.fromFiles({ name, files })`, `Skill.fromPath(rootDir, { name })`, and
`Skill.fromUrl(url, { name })` build an unstaged `Skill`. The instance carries
canonical zip bytes and a `sha256:<hex>` content hash.

```ts
import { AgentExecutor, RunModels, Skill } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken });

await aex.submitRun({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt,
  skills: [await Skill.fromPath("./skills/rules", { name: "rules" })],
  secrets: { apiKey }
});
```

Before it posts `/runs`, the SDK uploads each draft through the asset upload
flow:

1. `POST /assets/presign` checks for a dedup hit and, when needed, returns a
   signed R2 upload URL.
2. The SDK PUTs bytes directly to R2 with the signed checksum headers.
3. `POST /assets/finalize` confirms the object exists.

When direct R2 upload credentials are not configured, small bundles fall back to
the buffered `/assets` upload path. The runner re-verifies the content hash when
it downloads the asset.

## Pre-Upload For Reuse

If you want to build a local skill once and reuse the materialized asset across
multiple submissions, upload the draft explicitly:

```ts
const draft = await Skill.fromFiles({ name: "rules", files });
const uploaded = await draft.upload(aex);

await aex.submitRun({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt,
  skills: [uploaded],
  secrets: { apiKey }
});
```

The returned `uploaded` skill carries a plain `kind:"asset"` ref. Submitting it
does not upload bytes again.

## Fetch From A Signed URL

When your app runs in the cloud with limited local storage, host the skill
yourself as a zip archive with `SKILL.md` at the archive root and pass a
temporary signed URL:

```ts
const skill = await Skill.fromUrl(signedUrl, {
  name: "rules",
  sha256: "sha256:<hex>"
});
```

`Skill.fromUrl` fetches the archive in the SDK process. The hosted platform does
not fetch the caller-controlled URL. The signed URL only needs to be valid for
this call; the SDK snapshots the bytes into the asset store before the run is
submitted.

## Workspace Catalog

Catalog skills are workspace records backed by the same content-addressed R2
assets. Use them when a team wants a named, listed skill record:

```ts
const [record] = await aex.skills.list();

await aex.submitRun({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt,
  skills: [Skill.fromCatalog(record)],
  secrets: { apiKey }
});
```

The record must be `ready` and carry a content hash. `Skill.fromCatalog` performs
no upload; it produces a `kind:"asset"` ref directly against bytes already in
the catalog.
