---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/limits.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Limits
---

# Limits

The hosted service enforces account, workspace, session, file, telemetry, and
spend limits. Use typed API errors as the authority for an individual request.

Session compute is requested by capacity, not by selecting an implementation:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  compute: { size: "1gb" }
});
```

`compute.size` is the only compute selector. The session's
`resolvedConfig.compute` returns the provider-derived baseline and peak memory
and CPU, maximum disk, endpoint bandwidth, and concurrent-connection capacity.
Those derived fields are observable, but not independently selectable.

Read the adjustable limits currently effective for this workspace:

```ts
const limits = await aex.workspace.limits.list();
const queryPage = await aex.workspace.limits.get("query.page");
```

Each record includes its effective numeric value, whether it came from the
service default or a workspace override, its revision, and when it changed.
Provider-hard and protocol-hard bounds are not presented as workspace-adjustable
limits.

Long-running mutations return durable operation handles. A client timeout or
interrupt stops waiting; it does not imply that the server operation stopped.
