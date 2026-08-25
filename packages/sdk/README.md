# @aexhq/sdk

TypeScript SDK for Aex sessions and tool extensions.

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { openai } from "@aexhq/model-openai";
import { bash, read, task, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicrovm();

const session = await aex.sessions.create({
  model: {
    component: openai(),
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
  },
  agentloop: pi({ instructions: "Work carefully and verify changes." }),
  environments: { workspace },
  tools: [bash(), read(), write(), task()],
});

console.log(await session.send("Inspect the workspace."));
```

There is no default Agentloop, Model, Tool, or Environment. Every Tool is bound to one declared Environment before
the request is sent. An unbound tool is bound automatically only when exactly one declared
environment satisfies it; otherwise creation fails with the eligible or missing capabilities.

## Application Tools

Application callback authoring is temporarily outside the component MVP. Use precompiled Tool
component packages; the callback authoring API will return through the same Tool/Environment ABI,
without uploading function source to Brain.

The previous source-function builder shown below remains a donor until that replacement gate:

```ts
import { tool } from "@aexhq/sdk";
import { z } from "zod";

export default tool(
  z.object({ orderId: z.string() }),
  async function lookupOrder({ orderId }) {
    return database.lookup(orderId);
  },
)
  .describe("Look up an order")
  .returns(z.object({ status: z.string() }))
  .needs({ env: ["ORDERS_TOKEN"], network: [{ host: "orders.example.com", port: 443 }] })
  .setup(async function prepareIndex() {
    await database.prepareIndex();
  });
```

Use `tool.bind(environmentRef)` when more than one environment is compatible. Source tools run
through an `app()` callback environment. `aex tools build` prepares computer tools with their
runtime and dependencies; tool authors do not pass `import.meta.url` or choose a provider.

Environment references are opaque values returned by environment-extension factories. A created
session preserves their types, so `session.environment(ref)` returns that extension's typed handle.
Durable objects remain available through `session.storage`, independently of environment lifetime.
