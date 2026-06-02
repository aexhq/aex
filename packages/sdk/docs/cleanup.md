---
title: Cleanup
---

# Cleanup

antpath schedules cleanup after a run reaches a terminal status. There is no opt-out for antpath-owned cleanup attempts: tracked runtime resources such as Fly machines, scratch state, cached files, and run-scoped secret references are reclaimed or surfaced through `cleanupStatus` when cleanup cannot complete. The `cleanup.session` flag only controls the **provider-side** session deletion request (the Anthropic Managed Agents session, etc.).

By default antpath asks the provider/runtime to delete the provider session at terminal time. Opt into retention when you need it available for post-run inspection:

```ts
const runId = await client.submitRun({
  model: "claude-haiku-4-5",
  prompt: "...",
  secrets: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! } },
  cleanup: { session: "retain" }
});
```

The corresponding CLI flag is `--cleanup retain` (default `delete`).

A retained session remains visible on the provider until that provider's retention policy reclaims it. Antpath-owned cleanup attempts still run normally.

## Detecting retain mode

`cleanupStatus` does not encode the retain/delete request — it reports the state of our cleanup work. To check whether a run was submitted with retain mode, read it from the submission echo:

```ts
const run = await client.get(runId);
if (run.submission.cleanup?.session === "retain") {
  // provider session was retained
}
```

## `cleanupStatus` values

`cleanupStatus` reports the aggregate state of our cleanup work across the run's tracked resources. It is one of:

- `not_started` — terminal not yet reached, or no resources to clean.
- `pending` / `running` — cleanup is queued or in progress.
- `succeeded` — every resource was reclaimed.
- `failed_retryable` — a step failed in a way the cleanup worker will retry.
- `failed_terminal` — a step failed past retries; manual intervention may be needed.
- `skipped` — cleanup was deliberately not run for a particular resource (e.g. the provider session under `cleanup.session: "retain"`).
