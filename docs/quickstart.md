# Run your first agent on Aex

Aex hosts the session while your application supplies a model key and tools. This example looks up an order and prints the agent's answer.

## Get your keys

Use Node.js 24. Sign in at [aex.dev/dashboard](https://aex.dev/dashboard), create an API key, and save it when shown. Set `AEX_API_KEY` and `OPENAI_API_KEY` in your server environment. In PowerShell use `$env:AEX_API_KEY = "your-key"`. Keep both keys out of browsers and source control. Your model provider bills inference separately.

Accept the prices offered to your account and fund any required prepaid resources in **Account / Billing / Usage**. The example sets a hosting cost ceiling of 1 USD; this is a ceiling, not a price or a cap on your model-provider bill.

```sh
mkdir aex-example
cd aex-example
npm init -y
npm install @aexhq/sdk@1.0.0
```

## Create and send

Save this as `order.mjs`:

```js
import { Aex, ApiKey, Model, Tool } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({
    harness: "pi",
    model: Model("openai/gpt-4.1-mini", ApiKey(process.env.OPENAI_API_KEY)),
    tools: [Tool({
      name: "lookup_order",
      description: "Look up an order by id",
      input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"], additionalProperties: false },
      run: ({ id }) => ({ id, status: "shipped" }),
    })],
  });
  console.log((await session.send("Look up order A-1001. Has it shipped?")).output);
  console.log("Session:", session.id);
} finally {
  await aex.close();
}

```

Run `node order.mjs`. The lookup runs in your application and the selected harness runs on Aex. The answer's wording depends on the model. Replace the sample lookup with your own data.

`aex.close()` disconnects application callbacks and lets the process exit. It retains the hosted session. Save its ID and use `aex.sessions.get(id)` to read history or send more work. To restore callbacks after reconnecting, explicitly call `session.rebind(tools)` from the process that owns those functions. A disconnected process cannot execute tools.

`session.close()` stops the session and keeps its history. `session.delete()` removes session content and attachments after hosted resources have been cleaned up; a 409 means cleanup is still pending, so retry deletion. Neither operation silently replays an uncertain action.

## Choose what runs where

| Capability | Location | Guide |
| --- | --- | --- |
| Pi, Codex or OpenCode | Installed Aex native harness | [Brain harnesses](https://aex.dev/brain/docs) |
| `Tool({ run })` | Connected application process | [SDK](../packages/sdk/README.md) |
| Application endpoint | Your HTTPS service | [Application tools](http-tools.md) |
| DOM callbacks | Explicitly attached browser tab | [Browser tools](client-browser.md) |
| Sandbox, remote browser, inspection or MCP | Account-authorized service | [Managed tools](environments.md) |

Use [attachments](attachments.md) for images and PDFs, [typed responses](../examples/structured-output.mjs) for locally validated answers, and [billing](billing.md) to inspect charges and unknown usage.
