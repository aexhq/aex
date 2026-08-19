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

const result = await session.output(
  z.object({ answer: z.string() }),
  "Answer the question.",
);
```

`send()` returns text. `output()` returns a normal typed Promise and uses `https://api.aex.dev` by
default. Zod schemas must be representable as JSON Schema; process-local custom refinements and
transforms fail before any model work is admitted.

Sessions start with no tools. Select imported capabilities explicitly at creation:

```ts
import { computer, subagents } from "@aexhq/tools";

const session = await aex.sessions.create({
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    apiKey: "sk-ant-...",
  },
  tools: [computer(), subagents()],
});
```
