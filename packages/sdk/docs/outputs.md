---
title: Outputs
---

# Outputs

Every run produces durable metadata (status, events, snapshots, cleanup state) and an outputs namespace. By default, managed runs capture every regular file the run creates or modifies in the container: the runner snapshots the filesystem just before the agent starts, rescans it when the agent exits, and uploads the delta. There is no default or official output directory. Use `outputs.allowedDirs` only when you want to narrow capture to specific roots, and `outputs.deniedDirs` to subtract noise. `aex.download(runId)` returns the public run record — metadata, typed events, and captured output bytes — as a zip; the per-namespace verbs (`downloadOutputs` / `downloadEvents` / `downloadMetadata`) return one slice each.

## Quickstart

```ts
import { RunModels } from "@aexhq/sdk";

const runId = await aex.submit({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: "Produce a report and save it as a file.",
  secrets: { apiKeys: { anthropic: apiKey } }
});

await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

```bash
aex download <run-id> --out ./run.zip --api-token …
```

## The three namespaces

A run's downloadable content is organised into three logical namespaces, each with a matching verb. Every zip is assembled **client-side** from the public read endpoints (`getRun` + `listEvents` + `listOutputs` + per-output `/download`) — there is no server-side archive route.

| Namespace | What it holds | Verb | CLI |
| --- | --- | --- | --- |
| `outputs` | The run's real deliverables. | `downloadOutputs(runId)` | `download <id> --only outputs` |
| `events` | Typed event-channel records (`events.jsonl`). | `downloadEvents(runId)` | `download <id> --only events` |
| `metadata` | The run record (`run.json`). | `downloadMetadata(runId)` | `download <id> --only metadata` |

Platform diagnostics are stored outside the public archive under `runs/<runId>/internal/logs/` for internal/admin access only. They are not exposed by the SDK download helpers or the public CLI.

## What `download()` returns

`download(runId)` is the **whole-run** verb — it bundles the public namespaces as top-level folders. It is distinct from `downloadOutput(runId, selector)`, which fetches a single file. Layout:

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

The single-namespace verbs return the same per-file bytes at the zip root (e.g. `downloadOutputs(runId)` -> `report.txt` + a `manifest.json`; `downloadEvents(runId)` -> `events.jsonl`).

## Downloading one output

`downloadOutput(runId, selector)` returns a `Uint8Array`. Omit the selector to download the whole outputs namespace as a zip; pass an output from `aex.outputs(runId)`, an `{ id }`, or a path selector against the listed `Output.filename` values to download one file:

```ts
const allOutputs = await aex.downloadOutput(runId);
await aex.downloadOutput(runId, undefined, { to: "./outputs.zip" });

const report = await aex.downloadOutput(runId, { path: "reports/report.txt" });
console.log(new TextDecoder().decode(report));

const looseReport = await aex.downloadOutput(runId, { path: "report.txt", match: "suffix" });
console.log(looseReport.byteLength);
```

## Reading one output as text

`readOutputText(runId, selector, options?)` reads ONE output file as byte-capped, decoded UTF-8 text. It streams the file and stops at `options.maxBytes` (default 50 KB, ceiling 10 MB), so a large deliverable never fully buffers — this is the read built for handing a run's output to an LLM tool. Select the file the same way as `downloadOutput`: by `{ path }` (suffix-matchable) or `{ id }`.

```ts
const { text, truncated, totalBytes } = await aex.readOutputText(
  runId,
  { path: "report.md", match: "suffix" },
  { maxBytes: 50_000, grep: "error" }
);

if (truncated) {
  // text is a prefix of a larger file — narrow with `grep` or a tighter `path`.
}
```

Check `truncated` before treating `text` as complete. Pass `options.grep` (a substring or `RegExp`) to keep only matching lines of the capped text. The returned `output` is the matched `Output` record, and `totalBytes` is the file's full size when the server reports it.

### Chatting over a workspace's outputs

`createDataTools(client)` packages the read surface (`listRuns` + `listOutputs` + `readOutputText`) as a vendor-neutral LLM tool set (`{ tools, instructions, execute }`) so you can build a search-then-fetch chat over your runs and their outputs in a few lines on top of the public SDK. The `tools` are plain JSON-Schema definitions (the shape every major LLM tool API accepts); `execute(name, input)` dispatches a tool call against the workspace-scoped client. See the runnable `examples/data-chat/` example.

## Finding outputs

`listOutputs(runId, query?)` and its alias `outputs(runId, query?)` can filter the captured output list client-side. Use `findOutputs` when you want discovery to be explicit, or `findOutput` when exactly one file is expected:

```ts
const images = await aex.findOutputs(runId, { type: "image" });
const jsonReports = await aex.outputs(runId, {
  dir: "reports",
  extension: ".json"
});

const report = await aex.findOutput(runId, {
  filename: "summary.json",
  contentType: "application/json"
});
if (report) {
  const bytes = await aex.downloadOutput(runId, report);
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

`findOutput` returns `null` when nothing matches and throws `RunStateError` when the query matches more than one output.

## Temporary output links

Use `outputLink(runId, selectorOrQuery, options?)` when another process, browser, media tag, or downloader needs a direct artifact URL instead of bytes buffered through the SDK. `createOutputLink` remains as the compatibility name.

```ts
const link = await aex.outputLink(
  runId,
  { path: "reports/summary.json" },
  { expiresIn: "15m" }
);

console.log(link.url, link.expiresAt);
```

Selectors can be an output id, an `Output` object, a path selector, or an `OutputQuery`. `expiresIn` accepts seconds or `"15m"`, `"1h"`, or `"1d"`; the default is `"1h"`.

The returned URL is a reusable bearer URL until it expires. Anyone who has it can read that artifact during the TTL. aex does not promise one-time use or early revocation for these direct artifact URLs.

For large files, `fetchOutput` mints the same temporary URL and returns the `Response` from fetching it directly, without adding the SDK API token to that second request:

```ts
const response = await aex.fetchOutput(runId, { type: "video", filename: /clip\.mp4$/ });
const stream = response.body;
```

## Lifecycle behaviour

`download()` works at any run state — it reads whatever the public endpoints currently expose, so the zip reflects the run as of the call:

| Run state | Behaviour |
| --- | --- |
| `pending` / `queued` / `provisioning` | `metadata/run.json` reflects the early state; `events/` and `outputs/` are typically empty. |
| `provider_running`, mid-session / `cleaning_up` | Whatever events + outputs have been captured so far. Call again after terminal for the complete set. |
| `succeeded` / `failed` / `cancelled` / `terminated` | The complete typed event archive + all captured outputs. |

## `outputs.allowedDirs` — override capture roots

```ts
aex.submit({
  /* ... */,
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
aex.submit({
  /* ... */,
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

Mid-session calls are **best-effort and side-effect-free**: they expose whatever artifacts have already been uploaded. Files written by the agent are normally uploaded near terminal, after the filesystem diff. If you need the full output set, wait for the run to reach terminal status and call `download()` again.

## Safety

- Filenames are sanitized for cross-platform safety; collisions are disambiguated with a short id suffix before the extension.
- Downloads stay within the requested local directory.
- The archive endpoint is workspace-scoped (`outputs:read` scope) and rate-limited (`AEX_RATE_LIMIT_RUN_ARCHIVE_PER_MINUTE`, default 30/min/workspace).
- `manifest.json` never contains file bytes — only ids, paths, sizes, content types.
