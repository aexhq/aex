<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/aex-mark-white.webp" />
    <source media="(prefers-color-scheme: light)" srcset="./assets/aex-mark-black.webp" />
    <img src="./assets/aex-mark-black.webp" alt="" width="44" align="absmiddle" />
  </picture>
  Aex
</h1>

<p align="center">Run AI agents without operating an agent server.</p>

<p align="center">
  <a href="https://aex.dev/docs">Quickstart</a> ·
  <a href="https://aex.dev/dashboard">Dashboard</a> ·
  <a href="https://aex.dev/brain">Brain</a>
</p>

Aex hosts [Brain](https://github.com/aexhq/brain), the open-source server for AI agents.
Bring your model key, connect your tools, and send messages from your application.
Aex keeps the conversation and progress available for you to read later.

- Start with Brain's minimal core and compose the agent loop, tools and environments you need.
- Keep conversation history and failure evidence when a tool's execution environment fails.
- Choose where tools run, let Brain manage setup, and optionally give the model diagnostic tools.

Aex adds hosting, account authorization and usage tracking around these public interfaces.

> **Early preview.** APIs and limits may change. Maintenance can interrupt work;
> interrupted actions are not automatically retried. You pay your model provider separately.

## Get started

You need Node.js 22 or newer and an OpenAI API key. Sign in to the
[dashboard](https://aex.dev/dashboard) and create an Aex API key. Set `AEX_API_KEY` and
`OPENAI_API_KEY` in your server environment, then install:

```sh
npm install @aexhq/sdk@0.82.0 @aexhq/agentloop-pi@7.2.0 zod@4
```

Save this as `order.mjs`:

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
  const session = await aex.sessions.create({ environmentLifecycle: { default: "automatic" },
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

Run `node order.mjs`. The printed transcript includes the lookup result and an answer that
order A-1001 has shipped. Replace the sample lookup with your own data. The loop runs on Aex;
the lookup runs in your application, which must stay connected while its tools are needed.

For shell setup and more detail, see the [quickstart](docs/quickstart.md).

## Build your application

| Task | Guide |
| --- | --- |
| Continue a session, stream output or stop work | [Sessions](https://aex.dev/brain/docs/concepts/sessions) |
| Write a tool or customize the agent loop | [Brain guides](https://aex.dev/brain/docs/guides/write-a-tool) |
| Submit work from a short-lived request | [Managed tools and submission](docs/environments.md) |
| Choose setup policy and optional environment diagnostics | [Lifecycle and diagnostics](docs/environments.md#lifecycle-and-diagnostics) |
| Send an image or PDF | [Attachments](docs/attachments.md) |
| Get a typed JSON answer | [Structured output](https://aex.dev/brain/docs/guides/structured-output) |
| Manage keys and inspect usage in a terminal | [CLI](packages/cli/README.md) |
| Check prices, credits and spending | [Billing](docs/billing.md) |

## Hosting and costs

The dashboard shows the prices offered to your account before you accept them. Existing preview
accounts stay in preview until acceptance. Model hosting, managed compute and attachments have
separate usage charges; your model-provider bill remains separate.

Application tools can use your installed dependencies. Hosted tools and managed environments
have different access limits; see [where code runs](docs/quickstart.md#where-code-runs).

For self-hosting or contributions, see [Development](docs/development.md) and
[Operations](docs/operations.md). [MIT license](LICENSE).

Inline Tools default to the process registering them. Published libraries use Brain's generated
bindings and the selected Environment's prepared runtime. `ctx.finish(value, { content: summary })`
retains the structured result while offering concise model text. `ctx.model({ messages })` runs an
independent request within the invocation's lifetime and the session's model authority; hosted
usage and spending controls include those requests. The Agentloop owns the shared conversation.

Connect short application functions through an authenticated HTTP route with
[HTTP tools](https://github.com/aexhq/aex/blob/main/docs/http-tools.md). Submit a turn, close the
request and read its committed outcome later. Desktop apps can use
[upload-only grants](https://github.com/aexhq/aex/blob/main/docs/attachments.md#upload-from-a-desktop-without-an-account-key)
without receiving an account credential.
