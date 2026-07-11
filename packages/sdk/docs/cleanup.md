---
title: Cleanup
---

# Cleanup

aex owns runtime cleanup after a run ends. This internal work is not a public
session lifecycle state and is not represented by a synthetic `cleanupStatus`
field. A terminal RUN event means the run's checkpoint, files, billing, and
public read model are consistent; explicit session deletion remains a separate
operation.

The hosted product uses managed runtimes for all supported providers. Reusable
provider-session retention is not a supported session option, and the removed
retention field is rejected if supplied.

```ts
import { Models } from "@aexhq/sdk";

await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "...",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Session sequencing is exposed only through `currentRun` and `lastRun`. Use
`session.delete()` when the session itself should be deleted.
