# @aexhq/sdk

```ts
import { Aex } from "@aexhq/sdk";
import { z } from "zod";

const aex = new Aex({ apiKey: "aex_sk_..." });
const session = await aex.sessions.create({
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    apiKey: "sk-ant-...",
  },
});

const result = await session.send("Answer the question.", {
  output: z.object({ answer: z.string() }),
});
```

`send()` returns text by default. Passing `output` returns a normal typed Promise and uses
`https://api.aex.dev` by default. Zod schemas must be representable as JSON Schema; process-local
custom refinements and transforms fail before any model work is admitted.

Sessions start with no model tools. Configure the exact capabilities at creation:

```ts
import { bash, edit, read, subagents, write } from "@aexhq/tools";

const session = await aex.sessions.create({
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    apiKey: "sk-ant-...",
  },
  tools: [bash(), read(), write(), edit(), subagents()],
});
```

Omitting `tools` and passing `tools: []` are equivalent.
