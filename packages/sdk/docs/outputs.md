---
title: Outputs
---

# Outputs

Every run produces durable metadata (status, events, snapshots, cleanup state) and an outputs namespace. File capture is always attempted against a runtime-specific default directory; the submission's `outputDirs` field overrides that default when you want to capture additional or different paths. `client.download(runId)` returns the whole run — metadata, events, logs, and captured output bytes — as a zip; the per-namespace verbs (`downloadOutputs` / `downloadLogs` / `downloadEvents` / `downloadMetadata`) return one slice each.

> Inside the runtime, the primary output path is exposed as `$AEX_OUTPUTS` (sourceable from `RUNTIME.env`) and as `runtimeManifest.envVars.AEX_OUTPUTS` on the `Run` returned by `client.get(runId)`. Goose Managed defaults to `/workspace/outputs`.

## Quickstart

```ts
const runId = await client.submitRun({
  model: "claude-haiku-4-5",
  prompt: "Produce a report and stash working files under $AEX_OUTPUTS",
  secrets: { anthropic: { apiKey } }
});

await client.wait(runId);
await client.download(runId, { to: "./run.zip" });
```

```bash
aex download <run-id> --out ./run.zip --api-token …
```

## The four namespaces

A run's downloadable content is organised into four logical namespaces, each with a matching verb. Every zip is assembled **client-side** from the public read endpoints (`getRun` + `listEvents` + `listOutputs` + per-output `/download`) — there is no server-side archive route.

| Namespace | What it holds | Verb | CLI |
| --- | --- | --- | --- |
| `outputs` | The run's real deliverables. | `downloadOutputs(runId)` | `download <id> --only outputs` |
| `logs` | Platform diagnostics: `anthropic-debug/`, `goose-logs/`, `fly-logs/`. Stored under their own R2 prefix (`runs/<id>/logs/`), so `outputs` stays deliverables-only. | `downloadLogs(runId)` | `download <id> --only logs` |
| `events` | Typed events (`events.jsonl`) plus log/full-stream JSONL when the event channel opt-ins are available. | `downloadEvents(runId)` | `download <id> --only events` |
| `metadata` | The run record (`run.json`). | `downloadMetadata(runId)` | `download <id> --only metadata` |

## What `download()` returns

`download(runId)` is the **whole-run** verb — it bundles all four namespaces as top-level folders. It is distinct from `downloadOutput(runId, selector)`, which fetches a single file. Layout:

```
metadata/run.json     # run record (status, runId, timestamps, snapshot)
metadata/submission.json # public-safe submission snapshot, when available
metadata/cost.json    # public cost telemetry, when available
events/events.jsonl   # typed event-channel records, ordered
events/logs.jsonl     # log-channel records, when available
events/all.jsonl      # full unified stream, when available
outputs/<name>        # one file per deliverable
logs/<name>           # platform diagnostics (anthropic-debug/ …)
manifest.json         # RunRecordManifestV1
```

`manifest.json` is the versioned `RunRecordManifestV1` described in [Run record](run-record.md). It carries:

| Field | Meaning |
| --- | --- |
| `schemaVersion` / `runRecordSchemaVersion` | Manifest and run-record contract versions. |
| `runId` | The run the zip was assembled for. |
| `namespaces[]` / `files[]` | Namespace inventory and per-file presence state. Optional submission/cost/event-channel files are marked `present` only when the client assembled actual entries; custody remains `pending` until its writer/read path exists. |
| `outputs[]` / `logs[]` | `{ id, filename, sizeBytes?, contentType? }` — one row per file successfully written under `outputs/` / `logs/`. |
| `errors[]` | `{ namespace, id, filename, message }` — per-artifact byte fetches that failed during assembly. Best-effort: a failure records an entry here and is skipped from the tree rather than aborting the whole zip. |

The single-namespace verbs return the same per-file bytes at the zip root (e.g. `downloadOutputs(runId)` → `report.txt` + a `manifest.json`; `downloadEvents(runId)` → `events.jsonl` plus optional `logs.jsonl` / `all.jsonl`).

## Downloading one output

`downloadOutput(runId, selector)` returns a `Uint8Array`. Omit the selector to download the whole outputs namespace as a zip; pass an output from `client.outputs(runId)`, an `{ id }`, or a path selector against the listed `Output.filename` values to download one file:

```ts
const allOutputs = await client.downloadOutput(runId);
await client.downloadOutput(runId, undefined, { to: "./outputs.zip" });

const report = await client.downloadOutput(runId, { path: "reports/report.txt" });
console.log(new TextDecoder().decode(report));

const looseReport = await client.downloadOutput(runId, { path: "report.txt", match: "suffix" });
console.log(looseReport.byteLength);
```

## Lifecycle behaviour

`download()` works at any run state — it reads whatever the public endpoints currently expose, so the zip reflects the run as of the call:

| Run state | Behaviour |
| --- | --- |
| `pending` / `queued` / `provisioning` | `metadata/run.json` reflects the early state; `events/` and `outputs/` are typically empty. |
| `provider_running`, mid-session / `cleaning_up` | Whatever events + outputs have been captured so far. Call again after terminal for the complete set. |
| `succeeded` / `failed` / `cancelled` / `terminated` | The complete typed event archive + all captured outputs; log/full-stream JSONL are included when the deployed event API serves those channel opt-ins. |

## `outputDirs` — override capture roots

```ts
client.submitRun({
  /* ... */,
  outputDirs: ["/mnt/session/outputs", "/mnt/session/state"]
});
```

When omitted, aex captures the runtime default output directory. When supplied, `outputDirs` replaces that default with the listed paths.

Validation:

- absolute UNIX paths only (`/...`),
- no `..` segments, no NUL bytes,
- maximum 32 entries,
- maximum 512 bytes per entry.

Runtime notes:

- Goose Managed captures files by walking the configured directories in the runner container.
- Goose Managed captures by walking managed runtime directories directly.

Mechanism (no platform-magical paths — this is honest):

1. The hosted platform submits the run, sends the user prompt, streams events.
2. At session-idle (the agent's primary task is done), the platform sends one synthetic `user.message` to the agent:
   *"Run `node /mnt/session/uploads/aex/aex outputs sync <dirs>` once."*
3. Goose Managed captures by walking managed runtime directories directly.
4. The platform walks the Files API, copies bytes into durable output storage, and tears down the session.

Cost: one extra agent turn (~hundreds of tokens, observable in `span.model_request_*` events). Document this against your token budget if you submit very high-volume runs.

Capture failure modes — when the platform could not capture a file's bytes at all, the reason is surfaced on the run unit (`getRunUnit(runId).outputCaptureFailures`), not in the download zip. The zip's `manifest.errors[]` only records per-output *byte fetches* that failed while assembling the archive.

| `reason` | What happened |
| --- | --- |
| `agent_did_not_sync` | The agent refused or skipped the synthetic instruction. Run still succeeded, just no file bytes. |
| `agent_reported_error` | The `node /mnt/session/uploads/aex/aex outputs sync` invocation returned non-zero (e.g. dir did not exist). |
| `session_terminated_pre_sync` | Session was terminated (cancel / timeout) before the sync turn ran. |
| `storage_cap_exceeded` | Workspace storage quota would have been breached. |
| `download_failed` | Files API entry could not be fetched. |
| `pending_session_terminal` | Mid-session download — file may show up on a later `download()` once the session reaches terminal. |

## Runs without explicit `outputDirs`

Metadata still gets the full treatment. aex captures the runtime default output directory and returns whatever files exist there. A run that produces no files still returns a zip with `run.json`, `events.jsonl`, and an empty `outputs/` directory (manifest `outputs: []`).

## Mid-session download semantics

Mid-session calls are **best-effort and side-effect-free**: the platform exposes whatever the agent has already registered with the Files API. A mid-session call does **not** trigger a synthetic sync — that would interfere with the running agent's plan. If you need the full output set, wait for the run to reach terminal status and call `download()` again.

## Safety

- Filenames are sanitized for cross-platform safety; collisions are disambiguated with a short id suffix before the extension.
- Downloads stay within the requested local directory.
- The archive endpoint is workspace-scoped (`outputs:read` scope) and rate-limited (`AEX_RATE_LIMIT_RUN_ARCHIVE_PER_MINUTE`, default 30/min/workspace).
- `manifest.json` never contains file bytes — only ids, paths, sizes, content types.
