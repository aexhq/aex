# Aex TypeScript SDK

Run hosted agents with your own model key and application tools. Requires Node.js 24. Browser usage uses scoped access and an explicit user gesture.

```sh
npm install @aexhq/sdk@1.1.0
```

The [quickstart](../../docs/quickstart.md) includes a complete example. This SDK composes Brain 2.3.0's client and neutral contracts; Aex adds account policy, attachments, billing, browser grants and validated typed answers.

```js
import { Pi, Aex, ApiKey, Model } from "@aexhq/sdk";
const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({ harness: Pi(), model: Model("openai/gpt-4.1-mini", ApiKey(process.env.OPENAI_API_KEY)) });
  console.log((await session.send("Hello")).output);
} finally { await aex.close(); }
```

`send()` returns a turn; `submit()` returns accepted input. Use `events()` for SSE and `history(cursor)` for opaque-cursor pages. Reuse an idempotency key with identical input after a lost acknowledgement. Uncertain actions are not automatically repeated.

`Tool({name,description,input_schema,run})` runs in its connected declaring process. Return JSON from `run`; use its signal for cancellation. Explicitly `rebind(tools)` after reconnecting. `close()` disposes callback connections; session `close()` retains history and `delete()` waits for cleanup.

For structured output, install Zod 4.4.3 and pass `{output:{type:z.object(...),maxRetries:2}}` to `send`. The SDK validates the final assistant message and can request bounded corrective turns. Provider errors and missing answers fail explicitly. Corrective inference is billable.

[Application endpoints](../../docs/http-tools.md) · [Browser tools](../../docs/client-browser.md) · [Managed tools](../../docs/environments.md) · [Attachments](../../docs/attachments.md) · [Billing](../../docs/billing.md)
