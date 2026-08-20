# TypeScript quickstart

Create an Aex API key in the [dashboard](https://aex.dev/dashboard), then install the SDK and Zod:

```sh
npm install @aexhq/sdk zod
```

```ts
import { Aex } from "@aexhq/sdk";
import { z } from "zod";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "openai/gpt-5.4",
    apiKey: process.env.AI_GATEWAY_API_KEY!,
    baseUrl: "https://ai-gateway.vercel.sh",
  },
});

const result = await session.send("Review this repository.", {
  output: z.object({
    summary: z.string(),
    nextSteps: z.array(z.string()),
  }),
});

console.log(result.summary);
```

`session.send()` returns text by default. Passing `output` returns a typed Promise and validates
the result against the supplied schema. Call `session.send()` again to continue the same session.

The Aex client uses `https://api.aex.dev` by default. The model's `provider` selects its wire
protocol; `baseUrl` routes model calls through Vercel AI Gateway. These are independent settings.

Sessions start with no model tools. Install `@aexhq/tools` and pass only the capabilities the
application needs, such as `bash()`, `read()`, `write()`, `edit()`, or `subagents()`. Omitting
`tools` and passing `tools: []` are equivalent.

[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Control API](../contracts/control/v1/openapi.yaml)
