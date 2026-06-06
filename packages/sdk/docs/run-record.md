---
title: Run record
---

# Run record

The run record is the durable product primitive for one run id. It is the public-safe bundle of status metadata, the non-secret submission snapshot when available, typed events, captured outputs, and manifest entries for custody and cost telemetry.

`aex.download(runId)` and `aex download <run-id>` return a zip with this layout:

```text
manifest.json
metadata/run.json
metadata/submission.json      # when a public-safe submission snapshot is returned by the read API
metadata/cost.json            # when public cost telemetry is returned by the read API
events/events.jsonl
outputs/<captured deliverable files>
```

`manifest.json` is versioned as `RunRecordManifestV1`:

| Field | Meaning |
| --- | --- |
| `schemaVersion` | `aex.run-record.manifest.v1`. |
| `runRecordSchemaVersion` | `aex.run-record.v1`. |
| `runId` | The run the archive was assembled for. |
| `namespaces[]` | The documented top-level namespaces: `metadata`, `events`, `outputs`. |
| `files[]` | Inventory of expected and present files with `namespace`, `path`, `role`, and `status`. |
| `outputs[]` | Compatibility alias for present captured output artifacts. |
| `errors[]` | Per-artifact byte fetch failures during archive assembly. |

Current v1 downloads always include `metadata/run.json` and `events/events.jsonl`. `events/events.jsonl` contains typed event-channel records only; internal diagnostics and full internal streams are not mixed into that file.

`metadata/submission.json` is present only when the run read shape includes a public-safe submission snapshot. `metadata/cost.json` is present only when the run read shape includes public `costTelemetry`; otherwise cost stays `pending`. `metadata/custody.json` remains `pending` until the custody manifest writer and public read surface land. `events/manifest.json` remains `unavailable` in this client-side slice because there is no public coordinator-manifest download route.

The record boundary is public-safe. It must not contain provider API keys, runner bearers, workspace tokens, signed URLs, raw provider response bodies, object-store keys, Vault ids, raw query strings, secret-shaped values, or internal diagnostic files. Credentials are supplied per run and vaulted separately for the run lifetime.
