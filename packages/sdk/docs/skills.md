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
  persist on antpath as workspace skills with auto-suffixed names,
  one row per submission (see "Inline supply" below).

**Routing at session create:** bundles that contain `SKILL.md` at the
bundle root are registered with Anthropic's Skills API
(`POST /v1/skills`) and surface to the agent as auto-discoverable
skills. Bundles without `SKILL.md` mount under
`/mnt/session/uploads/antpath/assets/<skl_id>/<rel-path>` in the agent
container — the agent reads them by explicit path reference in the
prompt.

The platform also mounts the `antpath` CLI at
`/mnt/session/uploads/antpath/antpath` and a per-run manifest at
`/mnt/session/uploads/antpath/index.json` on **every** run. Skills
invoke the managed HTTP proxy via
`node /mnt/session/uploads/antpath/antpath proxy …` — see
`credentials.md` for the policy/auth model.

## Inline supply at `submitRun`

`Skill.fromFiles({name, files})` and `Skill.fromPath(rootDir, {name})`
build an **unstaged** `Skill`. The instance carries the canonicalised
zip bytes and the `sha256:<hex>` content hash:

```ts
import { AntpathClient, Skill } from "antpath";

const client = new AntpathClient({ apiToken });

await client.submitRun({
  model, prompt,
  skills: [await Skill.fromFiles({ name: "rules", files })],
  secrets: { anthropic: { apiKey } }
});
```

`client.submitRun` walks the `skills` array, sends a multipart body
alongside the JSON submission, and materializes the bytes to
content-addressable, workspace-scoped R2 storage before the run lands.
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
`client.skills.*` verbs.

### Deletion semantics

Soft-deleting a skill (`client.skills.delete(skl_id)` or the
dashboard's Delete button) marks the row tombstoned. Existing runs
that pinned a snapshot of the skill keep working — they read from
`run_skill_snapshots` which preserves the name, hash, size, and
manifest. The run detail view shows a "deleted" badge for those
orphan references; the download endpoint returns
**HTTP 410 Gone** with `{ error: { code: "skill_deleted", … } }`
when the caller tries to fetch the bytes of a deleted skill.

New `submitRun` calls referencing a soft-deleted `skl_*` id are
rejected before insertion.
