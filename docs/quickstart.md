# TypeScript quickstart

Install the hosted SDK and the extensions your session uses:

```sh
npm install @aexhq/sdk @aexhq/loop-pi @aexhq/tools @aexhq/env-aws-microvm
```

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ region: "eu-west-2" });
const session = await aex.createSession({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentLoop: pi(),
  tools: [read().runIn(workspace), write().runIn(workspace), bash().runIn(workspace)],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

Agentloop packages contain policy only. Tool implementations execute in their selected remote
Environment, never in Brain. Mutating methods generate operation keys automatically; pass an
explicit `idempotencyKey` only when retrying from caller-owned durable work.
