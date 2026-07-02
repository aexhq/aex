---
title: Outputs
---

# Outputs

Every session produces durable metadata (status, events, snapshots, cleanup state) and an outputs namespace. By default, managed runs capture every regular file the agent creates or modifies in the container: the runner snapshots the filesystem just before the agent starts, rescans it when the agent exits, and uploads the delta. There is no default or official output directory. Use `outputs.allowedDirs` only when you want to narrow capture to specific roots, and `outputs.deniedDirs` to subtract noise. `session.download()` returns the public session record — metadata, typed events, and captured output bytes — as a zip; the per-namespace verbs (`session.outputs().download()` / `session.events().download()` / `session.downloadMetadata()`) return one slice each.

The output verbs below hang off the session's `outputs()` accessor
(`session.outputs().list()`, `.read()`, `.download()`, …). Reach a handle from a
live session (`openSession` / `run`) or reopen one later with
`aex.openSession(sessionId)`; the client also exposes cross-session reads under
`aex.sessions.*`.

## Quickstart

```ts
import { Models } from "@aexhq/sdk";

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: apiKey }
});

await session.send("Produce a report and save it as a file.").done();
await session.wait();
await session.download({ to: "./session.zip" });
```

```bash
aex download <session-id> --out ./session.zip --api-token …
```

## The three namespaces

A session's downloadable content is organised into three logical namespaces, each with a matching verb. Every zip is assembled **client-side** from the public read endpoints (session record + `session.events().list()` + `session.outputs().list()` + per-output `/download`) — there is no server-side archive route.

| Namespace | What it holds | Verb | CLI |
| --- | --- | --- | --- |
| `outputs` | The session's real deliverables. | `session.outputs().download()` | `download <id> --only outputs` |
| `events` | Typed event-channel records (`events.jsonl`). | `session.events().download()` | `download <id> --only events` |
| `metadata` | The session record (`run.json`). | `session.downloadMetadata()` | `download <id> --only metadata` |

Platform diagnostics are stored outside the public archive under `runs/<runId>/internal/logs/` for internal/admin access only. They are not exposed by the SDK download helpers or the public CLI.

## What `session.download()` returns

`session.download()` is the **whole-session** verb — it bundles the public namespaces as top-level folders. It is distinct from `session.outputs().download(selector)`, which fetches a single file. Layout:

```
metadata/run.json     # run record (status, runId, timestamps, snapshot)
metadata/submission.json # public-safe submission snapshot, when available
metadata/cost.json    # public cost telemetry, when available
events/events.jsonl   # typed event-channel records, ordered
outputs/<name>        # one file per deliverable
manifest.json         # RunRecordManifestV1
```

`manifest.json` is the versioned `RunRecordManifestV1` described in [Run record](run-record.md). It carries:

| Field | Meaning |
| --- | --- |
| `schemaVersion` / `runRecordSchemaVersion` | Manifest and run-record contract versions. |
| `runId` | The run the zip was assembled for. |
| `namespaces[]` / `files[]` | Namespace inventory and per-file presence state. Optional submission/cost files are marked `present` only when the client assembled actual entries; custody remains `pending` until its writer/read path exists. |
| `outputs[]` | `{ id, filename, sizeBytes?, contentType? }` — one row per file successfully written under `outputs/`. |
| `errors[]` | `{ namespace, id, filename, message }` — per-artifact byte fetches that failed during assembly. Best-effort: a failure records an entry here and is skipped from the tree rather than aborting the whole zip. |

The single-namespace verbs return the same per-file bytes at the zip root (e.g. `session.outputs().download()` -> `report.txt` + a `manifest.json`; `session.events().download()` -> `events.jsonl`).

## Downloading one output

`session.outputs().download(selector)` returns a `Uint8Array`. Omit the selector to download the whole outputs namespace as a zip; pass an output from `session.outputs().list()`, an `{ id }`, or a path selector against the listed `Output.filename` values to download one file:

```ts
const allOutputs = await session.outputs().download();
await session.outputs().download(undefined, { to: "./outputs.zip" });

const report = await session.outputs().download({ path: "reports/report.txt" });
console.log(new TextDecoder().decode(report));

const looseReport = await session.outputs().download({ path: "report.txt", match: "suffix" });
console.log(looseReport.byteLength);
```

## Reading one output as text

`session.outputs().read(selector, options?)` reads ONE output file as byte-capped, decoded UTF-8 text. It streams the file and stops at `options.maxBytes` (default 50 KB, ceiling 10 MB), so a large deliverable never fully buffers — this is the read built for handing a session's output to an LLM tool. Select the file by `{ path }` (suffix-matchable) or `{ id }`. To read from a session id without a live handle, use `aex.sessions.outputs(sessionId).read(selector, options?)` — the same accessor, addressed by id.

```ts
const { text, truncated, totalBytes } = await session.outputs().read(
  { path: "report.md", match: "suffix" },
  { maxBytes: 50_000, grep: "error" }
);

if (truncated) {
  // text is a prefix of a larger file — narrow with `grep` or a tighter `path`.
}
```

Check `truncated` before treating `text` as complete. Pass `options.grep` (a substring or `RegExp`) to keep only matching lines of the capped text. The returned `output` is the matched `Output` record, and `totalBytes` is the file's full size when the server reports it.

## Finding outputs

`session.outputs().list(query?)` can filter the captured output list client-side. Use `session.outputs().find(query)` when you want discovery to be explicit, or `session.outputs().findOne(query)` when exactly one file is expected:

```ts
const images = await session.outputs().find({ type: "image" });
const jsonReports = await session.outputs().list({
  dir: "reports",
  extension: ".json"
});

const report = await session.outputs().findOne({
  filename: "summary.json",
  contentType: "application/json"
});
if (report) {
  const bytes = await session.outputs().download(report);
}
```

Query fields compose with AND semantics:

| Field | Match |
| --- | --- |
| `path` | Exact normalized output path. Leading `/` and `outputs/` are ignored. |
| `filename` | Basename match, as a string or `RegExp`. |
| `dir` / `recursive` | Directory prefix. `recursive` defaults to `true`; set `false` for direct children only. |
| `extension` | Case-insensitive extension, with or without a leading dot. |
| `contentType` | Exact content type or a prefix wildcard such as `image/*`. |
| `type` | High-level type: `text`, `json`, `image`, `audio`, `video`, `pdf`, `archive`, `binary`, or `unknown`. |

`session.outputs().findOne(query)` returns `null` when nothing matches and throws `RunStateError` when the query matches more than one output.

## Temporary output links

Use `session.outputs().link(selectorOrQuery, options?)` when another process, browser, media tag, or downloader needs a direct artifact URL instead of bytes buffered through the SDK.

```ts
const link = await session.outputs().link(
  { path: "reports/summary.json" },
  { expiresIn: "15m" }
);

console.log(link.url, link.expiresAt);
```

Selectors can be an output id, an `Output` object, a path selector, or an `OutputQuery`. `expiresIn` accepts seconds or `"15m"`, `"1h"`, or `"1d"`; the default is `"1h"`.

The returned URL is a reusable bearer URL until it expires. Anyone who has it can read that artifact during the TTL. aex does not promise one-time use or early revocation for these direct artifact URLs.

For large files, `session.outputs().fetch()` mints the same temporary URL and returns the `Response` from fetching it directly, without adding the SDK API token to that second request:

```ts
const response = await session.outputs().fetch({ type: "video", filename: /clip\.mp4$/ });
const stream = response.body;
```

## Lifecycle behaviour

`session.download()` works at any session state — it reads whatever the public endpoints currently expose, so the zip reflects the session as of the call:

| Run state | Behaviour |
| --- | --- |
| `pending` / `queued` / `provisioning` | `metadata/run.json` reflects the early state; `events/` and `outputs/` are typically empty. |
| `provider_running`, mid-session / `cleaning_up` | Whatever events + outputs have been captured so far. Call again after terminal for the complete set. |
| `succeeded` / `failed` / `cancelled` / `terminated` | The complete typed event archive + all captured outputs. |

## `outputs.allowedDirs` — override capture roots

```ts
aex.openSession({
  /* ... */
  outputs: {
    allowedDirs: ["/workspace/reports", "/workspace/state"]
  }
});
```

When omitted, aex captures the whole filesystem delta. When supplied, `outputs.allowedDirs` is a whitelist that replaces that default with the listed roots. In other words, explicit `outputs.allowedDirs` narrows capture; it does not add paths on top of `/`.

Validation:

- absolute UNIX paths only (`/...`),
- no `..` segments, no NUL bytes,
- maximum 32 entries,
- maximum 512 bytes per entry.

Runtime notes:

- The managed runtime captures files by diffing the filesystem against a baseline snapshot taken just before the agent starts. Platform setup files, installed packages, and materialized inputs are already present before the baseline, so they are excluded by timing.
- If you pass an explicit root that does not exist by terminal time, that root contributes no files.

## `outputs.deniedDirs` — subtract noise

```ts
aex.openSession({
  /* ... */
  outputs: {
    deniedDirs: ["node_modules", "/var/cache", "*.tmp"]
  }
});
```

`outputs.deniedDirs` is subtracted from the capture roots. Entries may be an absolute subtree (`/var/cache`), a bare path segment (`node_modules`), or a `*.ext` extension match. Denied entries beat allowed roots. Platform-mandatory excludes, including pseudo-filesystems and secret/platform paths, always apply and cannot be re-included.

Mechanism (no platform-magical paths — this is honest):

1. The hosted platform materializes the workspace, opens runtime logs, and records a filesystem baseline across the capture roots.
2. The agent runs normally. There is no extra model turn and no synthetic sync instruction.
3. When the agent exits, the runner rescans the capture roots and finds files that are new or whose metadata changed.
4. The runner uploads changed regular files to durable run artifact storage. Diagnostic log paths are routed to internal diagnostics under `runs/<runId>/internal/logs/`; other paths are routed to `outputs`.

Cost: output capture does not add a model turn. The runner pays a filesystem scan and upload cost near the end of the run.

Capture notes:

- Files over a configured per-file size cap are skipped.
- Once total file or byte caps are reached, remaining changed files are dropped from upload.
- Files that vanish between scan and upload are skipped.
- Upload failures are recorded in runner diagnostics. The zip's `manifest.errors[]` only records byte fetches that failed while assembling the download archive.

## Runs without explicit `outputs.allowedDirs`

Metadata still gets the full treatment. aex captures every regular file the run created or modified outside mandatory platform excludes. A run that produces no files still returns a zip with `run.json`, `events.jsonl`, and an empty `outputs/` directory (manifest `outputs: []`).

## Mid-session download semantics

Mid-session calls are **best-effort and side-effect-free**: they expose whatever artifacts have already been uploaded. Files written by the agent are normally uploaded near terminal, after the filesystem diff. If you need the full output set, wait for the session to park and call `session.download()` again.

## Safety

- Filenames are sanitized for cross-platform safety; collisions are disambiguated with a short id suffix before the extension.
- Downloads stay within the requested local directory.
- The archive endpoint is workspace-scoped (`outputs:read` scope) and rate-limited (`AEX_RATE_LIMIT_RUN_ARCHIVE_PER_MINUTE`, default 30/min/workspace).
- `manifest.json` never contains file bytes — only ids, paths, sizes, content types.
