---
title: Session record
---

# Session record

The session record is the durable product primitive for one session. It is the public-safe bundle of status metadata, the non-secret submission snapshot when available, typed events, captured outputs, and manifest entries for custody and cost telemetry.

## Listing sessions

`aex.sessions.list(query?)` enumerates the sessions in this workspace, most-recent first, one page at a time. The workspace is derived server-side from the API key, so this only ever returns your own sessions. It is the workspace-wide discovery entry point: combine it with `aex.sessions.outputs(id).list()` / `.read(...)` (see [Outputs](outputs.md)) to reach any session's deliverables.

```ts
let cursor: string | undefined;
do {
  const page = await aex.sessions.list({ status: "idle", limit: 25, cursor });
  for (const session of page.sessions) {
    console.log(session.id, session.status, session.createdAt, session.costUsd);
  }
  cursor = page.nextCursor;
} while (cursor);
```

`query` fields are all optional: `status` (single session status, e.g. `"idle"`), `since` (ISO-8601 lower bound on `createdAt`), `limit` (defaults to 25, clamped to `[1, 100]`), and `cursor` (the opaque keyset cursor from a prior page's `nextCursor` — absent on the last page). Each page row is a public-safe `SessionSummary` (`id`, `status`, `createdAt`, `updatedAt`, and `costUsd` once settled); it deliberately omits the submission snapshot (model / system / env). Use `aex.sessions.get(id)` for status / timing / cost on one session, or `session.unit()` on a handle for the full self-contained record including the parsed submission.

## Downloading a session record

`session.download()` and `aex download <session-id>` return a zip with this layout:

```text
manifest.json
metadata/session.json
metadata/submission.json      # when a public-safe submission snapshot is returned by the read API
metadata/cost.json            # when public cost telemetry is returned by the read API
events/events.jsonl
outputs/<captured deliverable files>
```

`manifest.json` is versioned as `SessionRecordManifestV1`:

| Field | Meaning |
| --- | --- |
| `schemaVersion` | `aex.session-record.manifest.v1`. |
| `sessionRecordSchemaVersion` | `aex.session-record.v1`. |
| `sessionId` | The session the archive was assembled for. |
| `namespaces[]` | The documented top-level namespaces: `metadata`, `events`, `outputs`. |
| `files[]` | Inventory of expected and present files with `namespace`, `path`, `role`, and `status`. |
| `outputs[]` | Compatibility alias for present captured output artifacts. |
| `errors[]` | Per-artifact byte fetch failures during archive assembly. |

Current v1 downloads always include `metadata/session.json` and `events/events.jsonl`. `events/events.jsonl` contains typed event-channel records only; internal diagnostics and full internal streams are not mixed into that file.

`metadata/submission.json` is present only when the session read shape includes a public-safe submission snapshot. `metadata/cost.json` is present only when the session read shape includes public `costTelemetry`; otherwise cost stays `pending`. `metadata/custody.json` remains `pending` until the custody manifest writer and public read surface land. `events/manifest.json` remains `unavailable` in this client-side slice because there is no public coordinator-manifest download route.

The record boundary is public-safe. It must not contain provider API keys, runner bearers, workspace tokens, signed URLs, raw provider response bodies, object-store keys, Vault ids, raw query strings, secret-shaped values, or internal diagnostic files. Credentials are supplied per session and vaulted separately for the session lifetime.
