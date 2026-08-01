---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/files.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
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

## Large and resumed downloads

One S3 `GetObject` can authorize at most 5,000,000,000,000 bytes. The
low-level `download()` methods therefore reject a selected range above that
provider-hard boundary. Plan a larger full or partial download without
allocating the object:

```ts
import { coordinateDownloadGrants } from "@aexhq/sdk";

const grants = coordinateDownloadGrants({
  sizeBytes: file.sizeBytes,
  sha256: file.sha256,
  mint: (range) => session.files.persisted.download({
    path: file.path, range
  })
});

for await (const { range, grant } of grants) {
  // Stream this exact signed range and verify grant.authorizedBytes.
}
```

Use `planDownloadRanges()` directly when only the arithmetic plan is needed.
The coordinator mints one bearer grant at a time and rejects any grant that
changes the planned range, object length, or immutable whole-object hash.

The CLI performs this coordination automatically. It streams each range into
`<output>.part`, verifies every authorized range length, verifies the complete
declared length and whole-object SHA-256, and renames atomically only after
completion. `DownloadGrant.sha256` is always the whole immutable object's hash,
not an arbitrary-range digest. A selected partial download remains bound to
that source hash but cannot independently match it until all object bytes are
assembled. `--resume` measures the existing `.part` file and requests only the
missing ranges; it never reads the partial object into memory.
