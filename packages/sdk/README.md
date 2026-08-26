# @aexhq/sdk

TypeScript SDK for Aex sessions and tool extensions.

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { bash, read, task, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicrovm();

const session = await aex.sessions.create({
  model: {
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

`model.provider` is a [models.dev](https://models.dev) provider id. Aex resolves it to the wire
dialect the engine speaks, the endpoint that speaks it, and the model's context window, and
refuses a provider it does not serve; `model.baseUrl` overrides the endpoint for a gateway or a
compatible server of your own, and must be https. Every session reads back the provider it
resolved.

There is no default Agentloop, Tool, or Environment. A Tool component that requests the
Environment capability requires the session's one declared Environment in the MVP.

## Application Tools

Application callbacks use the same Tool and Environment component ABI while the handler remains in
your process. Declare one `app()` Environment; Aex registers each selected callback over one
authenticated, reconnecting connection. Handler source and captured application state are never
uploaded to Brain.

```ts
import { Aex, tool } from "@aexhq/sdk";
import { app } from "@aexhq/env-app";
import { z } from "zod";

const lookupOrder = tool(
  z.object({ orderId: z.string() }),
  async function lookupOrder({ orderId }) {
    return database.lookup(orderId);
  },
)
  .describe("Look up an order")
  .returns(z.object({ status: z.string() }));

await new Aex({ apiKey: process.env.AEX_API_KEY! }).sessions.create({
  model,
  agentloop,
  environments: { application: app({ id: "orders-ui" }) },
  tools: [lookupOrder],
});
```

Source callback Tools are routed automatically to the single `app()` Environment. Durable objects
remain available through `session.storage`, independently of Environment lifetime.
