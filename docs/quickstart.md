# TypeScript quickstart

Install the hosted SDK and the extensions your session uses:

```sh
npm install @aexhq/sdk @aexhq/agentloop-pi @aexhq/tools @aexhq/env-aws-microvm
```

```ts
import { Aex, brainWasm } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ region: "eu-west-2" });
const session = await aex.sessions.create({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentloop: pi({ env: brainWasm() }),
  tools: [read({ env: workspace }), write({ env: workspace }), bash({ env: workspace })],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

The Agentloop owns model and compaction policy. Each Tool declares its execution Environment:
resident Tools stay in the app, native Components run in Brain, and packages such as these run in a
remote Environment. Mutating methods generate operation keys automatically; pass an explicit
`idempotencyKey` when the caller owns a stable operation identity.
