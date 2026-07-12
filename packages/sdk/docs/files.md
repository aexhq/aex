---
title: Files
---

# Files

There are two different file concepts:

- `aex.workspace.files`: reusable, versioned input resources.
- `session.files`: files captured from one session checkpoint.

Raw uploaded bytes are assets. A workspace file is a typed resource that pins
an asset, logical resource ID, version, hash, name, and mount path.

`mountPath` is always a destination directory. The runtime preserves each
source/archive filename inside that directory, so an `input.csv` published
with `mountPath: "/workspace/input"` appears at
`/workspace/input/input.csv`. A `name` passed to `File.fromPath` is only the
resource's storage slug; it never renames the mounted file. Multiple files can
intentionally share one mount directory as long as their resulting paths do
not collide.

## Reusable workspace files

```ts
import { File } from "@aexhq/sdk";

const source = await aex.workspace.files.publish(
  await File.fromPath("./data/input.csv", { mountPath: "/workspace/input" })
);

const session = await aex.sessions.create({
  model: "claude-haiku-4-5",
  assets: { files: [source] },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

`publish()` uploads immutable bytes, then creates or versions the workspace
resource. `list()`, `get(resourceId, version?)`, and `delete(resourceId)` manage
the catalog. Runs always receive pinned refs; they never resolve a mutable
"latest" version during execution.

As a run boots, each attached input archive is checked against a separate
runtime materialization envelope: at most 64 MiB compressed, 128 MiB expanded,
and 1,000 materialized files or safe symlinks. The runtime may overlap bulk skill extraction with
the first model call, but it blocks the first tool, any post-run hook, and run
completion until every promised input is ready. These execution-safety bounds
are distinct from the broader raw-asset storage quota. The SDK and workspace
publisher reject known violations before a resource can be pinned, and the
runtime revalidates the immutable archive before use. An invalid input fails explicitly;
tools and terminal results never observe a silently partial input tree.

## Session file snapshots

```ts
const result = await session.messages.send("Create reports/summary.md").finished();
const snapshot = await session.files.list({
  checkpointId: result.checkpoint?.checkpointId
});

console.log(snapshot.revision);
console.log(snapshot.files);
```

`SessionFilesSnapshot` contains both `revision` and `files`. Every file carries
the same `checkpointId` as the revision, plus its exact `sizeBytes` and
lowercase `sha256` digest. After `RUN_FINISHED`, this is the final committed
state for that run. During an active run, an older complete checkpoint may
still be visible.

## Find and read

```ts
const report = await session.files.findOne({
  filename: "summary.md",
  extension: "md"
});

if (report) {
  const preview = await session.files.read(report, { maxBytes: 50_000 });
  console.log(preview.text, preview.truncated);
}
```

Use `list(query?)`, `find(query)`, or `findOne(query)`. Cross-session file
search is not part of the public SDK. Open the owning session and query its
checkpoint instead.

## Download and links

```ts
const file = snapshot.files[0];
if (file) {
  const bytes = await session.files.download(file);
  const link = await session.files.link(file, { expiresIn: "15m" });
  const response = await session.files.fetch(file);
}
```

ID selectors must include their checkpoint:

```ts
await session.files.download({ id: "file_123", checkpointId: "cp_123" });
```

A string ID alone is intentionally rejected because an ID without a revision
can resolve inconsistently after a later run.

`download()` resolves the selector against that checkpoint and verifies both
the byte length and SHA-256 digest before returning. An integrity mismatch
fails immediately and is not retried. `read()` remains a bounded prefix read
for large files and verifies integrity when it consumes the complete file.
`link()` returns the resolved file metadata alongside the direct storage URL;
`fetch()` returns only the raw one-shot storage `Response`, so retain metadata
from `list()`, `findOne()`, or `link()` when independently verifying that stream.

Omit the selector from `session.files.download()` to download the files
namespace archive. `session.download()` returns the complete public session
archive containing metadata, events, and files.

## Failed runs

A `RUN_ERROR` can occur before any checkpoint exists. In that case the result
has `files: []` and `checkpoint === undefined`. A `RUN_FINISHED` without a
checkpoint is a contract error and the SDK fails loudly.
