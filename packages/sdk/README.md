# @aexhq/sdk

The hosted Aex client wraps the neutral `@aexhq/brain` HTTP SDK with an Aex API key and the
`https://api.aex.dev` base URL.

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  agentloop_digest,
  model: { binding_id: "vercel-ai-gateway", model: "openai/gpt-5.4" },
  presentation,
  environments,
  tool_bindings,
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

Admit Agentloop packages through `aex.brain.admitAgentloop(...)`. Tool implementations remain in
their bound remote Environment; the SDK sends definitions and binding requests, not implementation
code.
