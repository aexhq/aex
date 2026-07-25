---
title: Skills
---

# Skills

A skill is a `SKILL.md` bundle plus optional supporting files. Build a local
draft with a `Skill.from*` factory, publish it to the workspace, and attach the
returned immutable ref under `assets.skills`.

```ts
import { Skill } from "@aexhq/sdk";

const draft = await Skill.fromDir("./skills/report-writer", {
  name: "report-writer"
});
const reportWriter = await aex.workspace.skills.publish(draft);

const result = await aex.start({
  model,
  message: "Write the report.",
  assets: { skills: [reportWriter] }
});
```

Available draft factories:

- `Skill.fromDir(path, { name? })`
- `Skill.fromUrl(url, { name?, sha256?, timeoutMs?, fetch? })`
- `Skill.fromFiles({ name?, files, meta? })`
- `Skill.fromContent(skillMd, { name? })`
- `Skill.fromBytes({ name?, zip })`

Every bundle needs a root `SKILL.md` with a non-empty `description` in YAML
frontmatter. Names are validated locally, and archives are canonicalized and
content-hashed before publication.

## Immutable versions

Publication returns a `WorkspaceSkillRecord` containing a stable `resourceId`,
an immutable `version`, and the exact `assetId`/`contentHash`. Publishing new
bytes creates another version. Existing session submissions remain pinned to
their recorded version; they cannot silently observe later edits.

Drafts cannot be submitted or serialized directly. This keeps the only
promotion path visible and auditable:

```ts
const published = await aex.workspace.skills.publish(draft);
```

## Workspace administration

```ts
const page = await aex.workspace.skills.list({ limit: 100 });
const exact = await aex.workspace.skills.get(published.resourceId, published.version);
await aex.workspace.skills.delete(published.resourceId);
```

List calls return `{ resources, nextCursor? }`. `limit` defaults to 100 and
must be an integer from 1 through 100.

Skills are distinct from custom tools. The runtime exposes skills through its
skill-loading capability; executable custom functions belong in
`assets.tools`.
