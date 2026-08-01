---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: "CLI"
description: "Generated aex command-line reference."
---

# CLI

Generated from `aex --help`.

```text
aex — explicit v1 session CLI

Usage:
  aex account get
  aex organizations list|create|get|memberships list|invitations create
  aex workspaces list|create|get|delete
  aex api-keys list|create|revoke
  aex sessions create|list|get|stop|persist|fork|delete|credentials rebind
  aex messages list|send <sessionId>
  aex runs list|get <sessionId> [runId]
  aex operations list|get|wait|cancel
  aex workspace get|limits list|get|discard|files|skills|tools|instructions|mcp-servers|secrets|uploads
  aex workspace files download <name> --output <file|->
  aex files live|persisted list|stat|download
  aex approvals list|get|respond
  aex events|logs|spans|metrics|traces query|stream|listen [--session ID]
  aex telemetry query|stream|listen|gaps|export|download|revoke [--session ID]
  aex billing balance|usage|top-up|portal|auto-topup|statements

JSON input:
  --request <json|@file|->   mutation body
  --query <json|@file|->     observational query body

Common:
  --api-key KEY --aex-url URL --idempotency-key KEY --operation-id OP
  --if-revision N --detach --timeout-ms N --poll-interval-ms N
  --output <file|-> --force --resume
```
