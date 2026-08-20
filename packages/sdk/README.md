# `@aexhq/sdk`

TypeScript client for durable Aex sessions.

```ts
import { Aex } from "@aexhq/sdk";
import { z } from "zod";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const model = {
  provider: "openai" as const,
  name: "openai/gpt-5.4",
  apiKey: process.env.AI_GATEWAY_API_KEY!,
  baseUrl: "https://ai-gateway.vercel.sh",
};
const session = await aex.sessions.create({ model });

const result = await session.send("Answer the question.", {
  output: z.object({ answer: z.string() }),
});
```

`send()` returns text by default. Passing `output` returns a typed Promise. The client uses
`https://api.aex.dev` by default; the model `baseUrl` is independent of the Aex API origin.

Sessions start with no model tools. Add the exact capabilities needed at creation:

```ts
import { bash, edit, read, subagents, write } from "@aexhq/tools";

const session = await aex.sessions.create({
  model,
  tools: [bash(), read(), write(), edit(), subagents()],
});
```

Omitting `tools` and passing `tools: []` are equivalent. Zod schemas must be representable as JSON
Schema; process-local refinements and transforms fail before model work starts.
