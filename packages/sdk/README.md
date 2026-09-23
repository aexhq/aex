# Aex SDK

Run AI agents on [Aex](https://aex.dev) from JavaScript or TypeScript. Connect your model and
tools, send messages, and read saved conversations without operating a Brain server.

## Get started

Create a key in the [dashboard](https://aex.dev/dashboard). With Node.js 22 or newer, install:

```sh
npm install @aexhq/sdk@0.80.0 @aexhq/agentloop-pi@7.1.0 zod@4
```

Set `AEX_API_KEY` and `OPENAI_API_KEY` in your server environment. Save as `order.mjs` and
run `node order.mjs`:

```js
import { Aex, brainEnv, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

const lookupOrder = tool({
  name: "lookup_order",
  description: "Look up an order by id.",
  input: z.object({ id: z.string() }),
  run: ({ id }, ctx) => ctx.finish({ id, status: "shipped" }),
});

const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
try {
  const session = await aex.sessions.create({
    model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY },
    agentloop: pi({ env: brainEnv({ name: "brain" }) }),
    tools: [lookupOrder()],
  });
  try {
    await session.send("Look up order A-1001. Has it shipped?");
    console.log(JSON.stringify(await session.transcript(), null, 2));
    console.log("Session:", session.id);
  } finally {
    await session.end();
  }
} finally {
  await aex.close();
}
```

The transcript includes an order lookup and the agent's answer. The lookup function runs in your
process; keep it connected while the agent needs that tool. The [quickstart](https://aex.dev/docs)
explains setup and where other tools run.

## Sessions and tools

The SDK uses Brain's session API and extension helpers. You can import `tool`, `agentloop`,
`brainEnv`, `hostEnv`, `component` and `environment` from `@aexhq/sdk`.

- [Sessions](https://aex.dev/brain/docs/concepts/sessions): send, submit, stream, reconnect and stop.
- [Tools](https://aex.dev/brain/docs/guides/write-a-tool): application functions and packaged tools.
- [Structured output](https://aex.dev/brain/docs/guides/structured-output): validated JSON answers.
- [Managed environments](https://github.com/aexhq/aex/blob/main/docs/environments.md): hosted tools and workspaces.
- [Attachments](https://github.com/aexhq/aex/blob/main/docs/attachments.md): upload images and PDFs.

Use `ctx.finish(value)` to complete a tool. Use `aex.close()` in `finally` to close client
connections. Closing the client keeps stored sessions; tools in your process still require it.

## Account and usage

```js
console.log(await aex.account.get());
console.log(await aex.account.usage());
console.log(await aex.account.modelUsage(sessionId));
console.log(await aex.environments.list());
```

Run these before closing the client. Workload keys can read account and usage information.
Creating keys, accepting prices, topups and refunds require an account login; use the
[dashboard](https://aex.dev/dashboard) or [CLI](https://www.npmjs.com/package/@aexhq/cli).

See the [billing guide](https://github.com/aexhq/aex/blob/main/docs/billing.md) for account API
methods and `maxCostMicroUsd`, the ceiling for a resource operation. Aex spending controls do
not cap your separate model-provider bill.

Inline Tools default to the process registering them. Published libraries use Brain's generated
bindings and the selected Environment's prepared runtime. `ctx.finish(value, { content: summary })`
retains the structured result while offering concise model text. `ctx.model({ messages })` runs an
independent request within the invocation's lifetime and the session's model authority; hosted
usage and spending controls include those requests. The Agentloop owns the shared conversation.
