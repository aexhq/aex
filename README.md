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

Aex hosts [Brain](https://github.com/aexhq/brain), the open-source session server for AI agents. Bring your model key, connect tools, and send messages from your application. Aex keeps session history and adds account authorization, hosted services and usage billing.

Use native Pi, Codex or OpenCode. Tools can run in a connected application, an explicitly attached tab, your HTTPS service, or an account-authorized managed service.

## Get started

Use Node.js 24, create an API key in the [dashboard](https://aex.dev/dashboard), and set `AEX_API_KEY` and `OPENAI_API_KEY` on your backend.

```sh
npm install @aexhq/sdk@1.1.0
```

```js
import { Pi, Aex, ApiKey, Model, Tool } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({
    harness: Pi(),
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

The lookup runs in your application; the harness runs on Aex. Closing the client disconnects callbacks and keeps the session. Save its ID to continue later, explicitly rebind tools after reconnecting, or delete it to remove retained content after cleanup.

[Quickstart](docs/quickstart.md) · [SDK](packages/sdk/README.md) · [CLI](packages/cli/README.md) · [API](docs/api.md)

## Build your application

| Task | Guide |
| --- | --- |
| Run backend tools beyond a connected request | [Application endpoints](docs/http-tools.md) |
| Attach DOM callbacks with scoped access | [Browser tools](docs/client-browser.md) |
| Use a sandbox, remote browser or inspection tools | [Managed tools](docs/environments.md) |
| Send owned images and PDFs | [Attachments](docs/attachments.md) |
| Validate a typed answer | [Structured output example](examples/structured-output.mjs) |
| Inspect prices, credits and usage | [Billing](docs/billing.md) |

Your model provider bills inference separately. The dashboard shows hosting prices and prepaid requirements. Missing usage is reported as unknown. Maintenance can interrupt work; uncertain actions are never automatically retried.

## Develop and operate

[Development](docs/development.md) · [Operations and recovery](docs/operations.md) · [Roadmap](ROADMAP.md) · [Architecture decisions](docs/adr/README.md)

Aex consumes an immutable public Brain Go module and SDK. Product policy stays here; private deployment configuration and commercial choices stay in Platform. Brain 2 is a clean contract break with no legacy execution importer.
