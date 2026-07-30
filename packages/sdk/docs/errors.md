---
title: Errors
---

# Errors

All non-2xx API responses become `AexApiError` instances. The useful fields
are `status`, `apiCode`, `requestId`, and the redacted `body`.

```ts
import { AexApiError } from "@aexhq/sdk";

try {
  await aex.sessions.create({ model: "openai/gpt-5" });
} catch (error) {
  if (error instanceof AexApiError) {
    console.error(error.status, error.apiCode, error.requestId);
  }
  throw error;
}
```

Authentication, not-found, and idempotency conflicts have typed subclasses.
Long-running failures surface as `RunFailedError` or `OperationFailedError`
after `result()` observes a terminal record.
