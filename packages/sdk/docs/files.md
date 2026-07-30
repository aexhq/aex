---
title: Files
---

# Files

Session files have two explicit read surfaces:

```ts
const persisted = await session.files.persisted.list({
  path: "reports",
  recursive: true
});

const live = await session.files.live.stat({
  path: "reports/latest.txt",
  wake: "retained",
  consistency: "coherent"
});
```

Downloads mint a short-lived grant. Use the returned URL and headers once,
before `expiresAt`, and do not print the URL.

```ts
const persistedGrant = await session.files.persisted.download({
  path: "reports/latest.txt"
});

const liveGrant = await session.files.live.download({
  path: "reports/latest.txt",
  wake: "retained",
  consistency: "coherent",
  ifGenerationId: live.workspaceAccess.generationId
});
```

Registered workspace files have a separate current-name download:

```ts
const currentGrant = await aex.workspace.files.download(
  "repository-context",
  { range: { start: 0, endExclusive: 4096 } },
  { idempotencyKey: "download-repository-context" }
);
```

That call maps to
`POST /api/workspace/files/{name}/downloads`. The grant is bound to the exact
current name, revision, content SHA-256, and requested range at admission. A
later overwrite cannot retarget an already-issued grant, and old bytes have no
addressable read route.

The live response includes `workspaceAccess`, which identifies the generation
that answered the request. Persisted reads never wake a workspace.
