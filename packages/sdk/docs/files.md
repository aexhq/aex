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

The live response includes `workspaceAccess`, which identifies the generation
that answered the request. Persisted reads never wake a workspace.
