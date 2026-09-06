# TypeScript quickstart

Install the hosted SDK and the extensions your session uses:

```sh
npm install @aexhq/sdk @aexhq/agentloop-pi @aexhq/tools @aexhq/env-aws-microvm
```

```ts
import { Aex, brainEnv } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ name: "sandbox", url: process.env.ENVIRONMENT_URL, token: process.env.ENVIRONMENT_TOKEN, region: "eu-west-2" });
const session = await aex.sessions.create({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
  tools: [read({ env: workspace }), write({ env: workspace }), bash({ env: workspace })],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

The Agentloop owns model and compaction policy. Each Tool declares its execution Environment:
resident Tools stay in the app, native Components run in Brain, and packages such as these run in a
remote Environment. Mutating methods generate operation keys automatically; pass an explicit
`idempotencyKey` when the caller owns a stable operation identity.

This example requires a separately deployed AWS Environment driver. Supply its URL and bearer
credential as `ENVIRONMENT_URL` and `ENVIRONMENT_TOKEN`; the SDK keeps the credential out of the
Environment configuration and Tool implementation descriptors. See the
[Environment package](https://github.com/aexhq/extensions/tree/main/packages/env-aws-microvm)
for its supported runtime and deployment boundary.
