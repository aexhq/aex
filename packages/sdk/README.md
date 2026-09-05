# @aexhq/sdk

The hosted Aex client uses the neutral Brain composition API with the `https://api.aex.dev` base
URL and an Aex API key.

```ts
import { Aex, brainWasm } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ region: "eu-west-2" });
const session = await aex.sessions.create({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentloop: pi({ env: brainWasm() }),
  tools: [read({ env: workspace }), bash({ env: workspace })],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

Aex supplies hosted authentication and policy. Brain admission, Tool placement, Environment
attachments, operation keys, and event cursors are handled by the shared Brain SDK.
