---
title: Limits
---

# Limits

The hosted service enforces account, workspace, session, file, telemetry, and
spend limits. Use typed API errors as the authority for an individual request.

Session compute is requested by capacity, not by selecting an implementation:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  compute: {
    requestedSize: "1gb",
    peakSize: "4gb",
    diskSize: "16gb"
  }
});
```

Long-running mutations return durable operation handles. A client timeout or
interrupt stops waiting; it does not imply that the server operation stopped.
