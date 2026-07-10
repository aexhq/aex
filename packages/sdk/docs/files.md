---
title: Files
---

# Files

Every session produces durable metadata (status, events, cleanup state) and a files namespace backed by the latest complete checkpoint of the session workspace. By default, managed sessions expose regular workspace files from that checkpoint, EXCLUDING the inputs the platform itself materialized for you (your mounted `files`/`skills`) — those are excluded by IDENTITY (their exact destination paths and skill-dir prefixes), not by any before/after timing comparison. There is no default or official capture directory. Use `fileCapture.allowedDirs` only when you want to narrow the visible file roots, and `fileCapture.deniedDirs` to subtract noise. `session.download()` returns the public session record — metadata, typed events, and captured file bytes — as a zip; the per-namespace verbs (`session.files().download()` / `session.events().download()` / `session.downloadMetadata()`) return one slice each.

The file verbs below hang off the session's `files()` accessor
(`session.files().list()`, `.read()`, `.download()`, …). Reach a handle from a
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
npx aex download <session-id> --out ./session.zip --api-key …
```

## The three namespaces

A session's downloadable content is organised into three logical namespaces, each with a matching verb. Every zip is assembled **client-side** from the public read endpoints (session record + `session.events().list()` + `session.files().list()` + per-file `/download`) — there is no server-side archive route.

| Namespace | What it holds | Verb | CLI |
| --- | --- | --- | --- |
| `files` | The session's real deliverables. | `session.files().download()` | `download <id> --only files` |
| `events` | Typed event-channel records (`events.jsonl`) plus a namespace manifest. | `session.events().download()` | `download <id> --only events` |
| `metadata` | The session record (`session.json`) plus a namespace manifest. | `session.downloadMetadata()` | `download <id> --only metadata` |

Platform diagnostics are stored outside the public archive under `sessions/<sessionId>/internal/logs/` for internal/admin access only. They are not exposed by the SDK download helpers or the public CLI.

## What `session.download()` returns

`session.download()` is the **whole-session** verb — it bundles the public namespaces as top-level folders. It is distinct from `session.files().download(selector)`, which fetches a single file. Layout:

```
metadata/session.json     # session record (status, sessionId, timestamps, snapshot)
metadata/submission.json # public-safe submission snapshot, when available
metadata/cost.json    # public cost telemetry, when available
events/events.jsonl   # typed event-channel records, ordered
files/<name>        # one file per deliverable
manifest.json         # SessionRecordManifestV1
```

`manifest.json` is the versioned `SessionRecordManifestV1` described in [Session record](session-record.md). It carries:

| Field | Meaning |
| --- | --- |
| `schemaVersion` / `sessionRecordSchemaVersion` | Manifest and session-record contract versions. |
| `sessionId` | The session the zip was assembled for. |
| `namespaces[]` / `files[]` | Namespace inventory and per-file presence state. Optional submission/cost files are marked `present` only when the client assembled actual entries; custody remains `pending` until its writer/read path exists. |
| `sessionFiles[]` | `{ id, filename, sizeBytes?, contentType? }` — one row per file successfully written under `files/`. |
| `errors[]` | `{ namespace, id, filename, message }` — per-artifact byte fetches that failed during assembly. Best-effort: a failure records an entry here and is skipped from the tree rather than aborting the whole zip. |

The single-namespace verbs return the same per-file bytes at the zip root (e.g. `session.files().download()` -> `report.txt` + `manifest.json`; `session.events().download()` -> `events.jsonl` + `manifest.json`; `session.downloadMetadata()` -> `session.json` + `manifest.json`).

## Downloading one file

`session.files().download(selector)` returns a `Uint8Array`. Omit the selector to download the whole files namespace as a zip; pass a file from `session.files().list()`, an `{ id }`, or a path selector against the listed `SessionFile.filename` values to download one file:

```ts
const allFiles = await session.files().download();
await session.files().download(undefined, { to: "./files.zip" });

const report = await session.files().download({ path: "reports/report.txt" });
console.log(new TextDecoder().decode(report));

const looseReport = await session.files().download({ path: "report.txt", match: "suffix" });
console.log(looseReport.byteLength);
```

## Reading one file as text

`session.files().read(selector, options?)` reads ONE file as byte-capped, decoded UTF-8 text. It streams the file and stops at `options.maxBytes` (default 50 KB, ceiling 10 MB), so a large deliverable never fully buffers — this is the read built for handing a session file to an LLM tool. Select the file by `{ path }` (suffix-matchable) or `{ id }`. To read from a session id without a live handle, use `aex.sessions.files(sessionId).read(selector, options?)` — the same accessor, addressed by id.

```ts
const { text, truncated, totalBytes } = await session.files().read(
  { path: "report.md", match: "suffix" },
  { maxBytes: 50_000, grep: "error" }
);

if (truncated) {
  // text is a prefix of a larger file — narrow with `grep` or a tighter `path`.
}
```

Check `truncated` before treating `text` as complete. Pass `options.grep` (a substring or `RegExp`) to keep only matching lines of the capped text. The returned `file` is the matched `SessionFile` record, and `totalBytes` is the file's full size when the server reports it.

## Finding files

`session.files().list(query?)` can filter the captured file list client-side. Use `session.files().find(query)` when you want discovery to be explicit, or `session.files().findOne(query)` when exactly one file is expected:

```ts
const images = await session.files().find({ type: "image" });
const jsonReports = await session.files().list({
  dir: "reports",
  extension: ".json"
});

const report = await session.files().findOne({
  filename: "summary.json",
  contentType: "application/json"
});
if (report) {
  const bytes = await session.files().download(report);
}
```

Query fields compose with AND semantics:

| Field | Match |
| --- | --- |
| `path` | Exact normalized file path. Leading `/` and `files/` are ignored. |
| `filename` | Basename match, as a string or `RegExp`. |
| `dir` / `recursive` | Directory prefix. `recursive` defaults to `true`; set `false` for direct children only. |
| `extension` | Case-insensitive extension, with or without a leading dot. |
| `contentType` | Exact content type or a prefix wildcard such as `image/*`. |
| `type` | High-level type: `text`, `json`, `image`, `audio`, `video`, `pdf`, `archive`, `binary`, or `unknown`. |

`session.files().findOne(query)` returns `null` when nothing matches and throws `SessionStateError` when the query matches more than one file.

## Searching files

Search is metadata-only (reference hits — filename / extension / content type — no bytes). `filename` accepts a `string` (case-insensitive substring) or a `RegExp`. A content-shaped query (`content`/`text`/…) throws a typed "content search unsupported" rather than silently returning zero hits.

```ts
// One session's files:
const hits = await session.files().search({ filename: /report/i });

// Across every session in the workspace (or scope with sessionIds):
const all = await aex.files.search({ extension: "md", sessionIds: ["session-a", "session-b"] });
```

Each hit is `{ sessionId, fileId, filename?, sizeBytes?, contentType? }`; read the bytes with `session.files().read(...)` / `.download(...)`.

## CLI

The `aex files` verb is a thin pass-through over the same SDK accessor, so every per-file operation has a subcommand (`npx aex` on a local install):

```bash
npx aex files <session-id>                          # list captured files (NDJSON)
npx aex files read <session-id> <path>              # read one file as capped text (JSON)
npx aex files download <session-id> <path> --out f  # download one file's raw bytes
npx aex files link <session-id> <path>              # mint a temporary download URL (JSON)
npx aex files find <session-id> --name S --ext E --type T
npx aex files search --query S --ext E --session-id ID  # cross-session metadata search
```

`aex files search` (no session id) is the cross-session search (`aex.files.search`); the whole-namespace zip stays `aex download <session-id>`.

## Temporary file links

Use `session.files().link(selectorOrQuery, options?)` when another process, browser, media tag, or downloader needs a direct artifact URL instead of bytes buffered through the SDK.

```ts
const link = await session.files().link(
  { path: "reports/summary.json" },
  { expiresIn: "15m" }
);

console.log(link.url, link.expiresAt);
```

Selectors can be a file id, a `SessionFile` object, a path selector, or a `SessionFileQuery`. `expiresIn` accepts seconds or `"15m"`, `"1h"`, or `"1d"`; the default is `"1h"`.

The returned URL is a reusable bearer URL until it expires. Anyone who has it can read that artifact during the TTL. aex does not promise one-time use or early revocation for these direct artifact URLs.

For large files, `session.files().fetch()` mints the same temporary URL and returns the `Response` from fetching it directly, without adding the SDK API key to that second request:

```ts
const response = await session.files().fetch({ type: "video", filename: /clip\.mp4$/ });
const stream = response.body;
```

## Lifecycle behaviour

`session.download()` works at any session state — it reads whatever the public endpoints currently expose, so the zip reflects the session as of the call:

| SessionRecord state | Behaviour |
| --- | --- |
| `queued` / `claiming` / `provisioning` | `metadata/session.json` reflects the early state; `events/` and `files/` are typically empty. |
| `provider_running`, mid-session / `capturing_files` / `cleaning_up` | Whatever events + files have been captured so far. Call again after the session parks for the complete set. |
| `idle` / `suspended` (parked between turns) | The complete archive for every turn sent so far; a later turn appends to it. |
| `succeeded` / `failed` / `timed_out` / `cancelled` | The complete typed event archive + all captured files. |

## `fileCapture.allowedDirs` — override capture roots

```ts
aex.openSession({
  /* ... */
  fileCapture: {
    allowedDirs: ["/workspace/reports", "/workspace/state"]
  }
});
```

When omitted, aex exposes the session workspace files from the latest complete checkpoint, subject to mandatory platform excludes. When supplied, `fileCapture.allowedDirs` is a whitelist that replaces that default with the listed roots. In other words, explicit `fileCapture.allowedDirs` narrows file capture; it does not add paths on top of `/`.

Validation:

- absolute UNIX paths only (`/...`),
- no `..` segments, no NUL bytes,
- maximum 32 entries,
- maximum 512 bytes per entry.

Runtime notes:

- The managed runtime exposes regular files from the latest complete checkpoint under the capture roots, EXCLUDING the inputs the platform itself materialized (your mounted `files`/`skills`) by IDENTITY — their exact destination paths and skill-dir prefixes are threaded into the capture filter, so an untouched mounted input is never re-emitted as a captured file regardless of path policy or timing.
- If you pass an explicit root that does not exist by terminal time, that root contributes no files.

## `fileCapture.deniedDirs` — subtract noise

```ts
aex.openSession({
  /* ... */
  fileCapture: {
    deniedDirs: ["node_modules", "/var/cache", "*.tmp"]
  }
});
```

`fileCapture.deniedDirs` is subtracted from the capture roots. Entries may be an absolute subtree (`/var/cache`), a bare path segment (`node_modules`), or a `*.ext` extension match. Denied entries beat allowed roots. Platform-mandatory excludes, including pseudo-filesystems and secret/platform paths, always apply and cannot be re-included.

Mechanism (no platform-magical paths — this is honest):

1. The hosted platform materializes the workspace (your mounted `files`/`skills`) and records the exact destination paths + skill-dir prefixes it wrote.
2. The agent sessions normally. There is no extra model turn and no synthetic sync instruction.
3. At a complete checkpoint, the runner scans the capture roots and drops any file whose path is a materialized INPUT (exact path or under a materialized skill dir) — inputs are excluded by WHO PUT THEM THERE (the platform), not by timing.
4. The runner uploads the remaining regular files to durable session file storage. Diagnostic log paths are routed to internal diagnostics under `sessions/<sessionId>/internal/logs/`; other paths are routed to `files`.

Cost: file capture does not add a model turn. The runner pays a filesystem scan and upload cost near the end of the session.

Capture notes:

- Files over a configured per-file size cap are skipped.
- Once total file or byte caps are reached, remaining changed files are dropped from upload.
- Files that vanish between scan and upload are skipped.
- Upload failures are recorded in runner diagnostics. The zip's `manifest.errors[]` only records byte fetches that failed while assembling the download archive.

## Sessions without explicit `fileCapture.allowedDirs`

Metadata still gets the full treatment. aex exposes every regular workspace file from the latest complete checkpoint outside mandatory platform excludes. A session that produces no files still returns a whole-session zip with `metadata/session.json`, `events/events.jsonl`, and `manifest.json` (manifest `files: []`).

## Mid-session download semantics

Mid-session calls are **best-effort and side-effect-free**: they expose whatever files from a complete checkpoint have already been uploaded. Files written by the agent are normally uploaded after a complete checkpoint, once the runner scans the capture roots. If you need the full file set, wait for the session to park and call `session.download()` again.

## Safety

- Filenames are sanitized for cross-platform safety; collisions are disambiguated with a short id suffix before the extension.
- Downloads stay within the requested local directory.
- The archive endpoint is workspace-scoped (`files:read` scope) and rate-limited (`AEX_RATE_LIMIT_SESSION_ARCHIVE_PER_MINUTE`, default 30/min/workspace).
- `manifest.json` never contains file bytes — only ids, paths, sizes, content types.
