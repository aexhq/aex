# `@aexhq/tools`

Explicit tool selections for Aex sessions. Nothing is granted by default.

```ts
import { Aex } from "@aexhq/sdk";
import { bash, edit, read, subagents, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "openai/gpt-5.4",
    apiKey: process.env.AI_GATEWAY_API_KEY!,
    baseUrl: "https://ai-gateway.vercel.sh",
  },
  tools: [bash(), read(), write(), edit(), subagents()],
});
```

`glob()`, `grep()`, `ls()`, `todo()`, `webSearch()`, and `webFetch()` are separate opt-ins.
Duplicate selections fail before session creation.

`subagents()` enables the `task` tool. Each call runs one child agent inside the parent turn with
isolated history and the parent's model, workspace, and enabled tools (except `todo`). Calls from
one model message can run in parallel. Nesting, child count, cancellation, and replay are bounded
by the session engine.
