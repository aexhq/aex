---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/resources.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Registered resources
---

# Registered resources

Workspace files, skills, tools, instructions, and MCP servers are addressed by
name. `set()` is an overwrite of that name. It returns the outcome and the
current resource:

```ts
const result = await aex.workspace.instructions.set(
  "review-policy",
  { text: "Check tests before reporting completion." },
  {
    ifRevision: 3,
    idempotencyKey: "review-policy-update"
  }
);

result.status; // "created" | "replaced" | "unchanged"
result.resource.revision;
```

`unchanged` means the normalized value already matched. Its revision and
timestamps do not move. There is no historical, copy, publish, archive, or
restore resource behind the current name.

Sessions refer to registered names:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  registered: {
    instructions: ["review-policy"],
    files: ["repository-context"]
  }
});
```

Blob-backed registrations accept inline bytes or a completed workspace upload.
`BlobInput` exists only on writes:

```ts
await aex.workspace.files.set("repository-context", {
  mountPath: "context.txt",
  content: {
    type: "inline",
    encoding: "utf8",
    data: "current context",
    sha256: "sha256:..."
  },
  mediaType: "text/plain",
  mode: "0644"
});
```

Reads and PUT results never return inline data or an upload ID. Their byte field
is a `BlobDescriptor` containing only `{sha256,sizeBytes}`.

Large values use disposable uploads. Every part grant request declares the
part's exact byte length and checksum, and completion repeats that evidence
alongside the returned ETag:

```ts
await aex.workspace.uploads.parts(upload.id, {
  parts: [{ partNumber: 1, sizeBytes: part.byteLength, sha256: partSha256 }]
});

await aex.workspace.uploads.complete(upload.id, {
  parts: [{
    partNumber: 1,
    etag,
    sizeBytes: part.byteLength,
    sha256: partSha256
  }]
});
```

Registry `list()` is the pagination exception: it uses a current-view name
keyset, not a frozen snapshot. Deletes disappear, replacements may be observed,
and a name inserted at or before the cursor boundary may be missed. The opaque
24-hour cursor is bound to the registry and last emitted name, but callers may
change `limit` between pages.

Secrets have a separate metadata-only registry; secret values are never
returned after creation.
