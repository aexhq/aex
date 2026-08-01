---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/billing.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Account and billing
---

# Account and billing

Bootstrap clients expose account state and organization billing:

```ts
const state = await aex.account.get();
const balance = await aex.billing.balance.get({ organizationId });
const usage = await aex.billing.usage.query({
  timeRange: { gte: from, lt: to },
  groupBy: ["category"]
});
```

An account in `paused` state rejects new billable work. SDK failures preserve
the HTTP status, stable `apiCode`, and request ID on `AexApiError`; callers
should branch on those fields instead of matching message text.
