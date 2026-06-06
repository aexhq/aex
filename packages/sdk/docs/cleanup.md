---
title: Cleanup
---

# Cleanup

aex schedules cleanup after a run reaches a terminal status. There is no
opt-out for aex-owned cleanup attempts: tracked runtime resources such as
managed runtime machines, scratch state, cached files, and run-scoped secret
references are reclaimed when possible or surfaced through `cleanupStatus` when
cleanup cannot complete.

The hosted product uses managed runtimes for all supported providers. Reusable
provider-session retention is not a supported run option, and the removed
retention field is rejected if supplied.

```ts
const runId = await aex.submitRun({
  model: "claude-haiku-4-5",
  prompt: "...",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

## `cleanupStatus` values

`cleanupStatus` reports the aggregate state of our cleanup work across the
run's tracked resources. It is one of:

- `not_started` - terminal not yet reached, or no resources to clean.
- `pending` / `running` - cleanup is queued or in progress.
- `succeeded` - tracked cleanup work completed for the resources aex
  controls.
- `failed_retryable` - a step failed in a way the cleanup worker will retry.
- `failed_terminal` - a step failed past retries; manual intervention may be
  needed.
- `skipped` - cleanup was not applicable for a tracked resource, or the
  resource was already absent.
