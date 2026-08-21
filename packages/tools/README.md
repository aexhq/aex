# `@aexhq/tools`

Explicit execution-capability selections for Aex sessions. Nothing effectful is granted by default;
the SDK's reserved structured-output protocol is inert unless one `send({ output })` request arms it.

```ts
import { Aex } from "@aexhq/sdk";
import { bash, edit, read, sandbox, storage, subagents, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
  },
  tools: [bash(), read(), write(), edit(), storage(), sandbox(), subagents()],
});
```

`storage()` gives the model one action-discriminated Tool for explicit save/load/list operations;
the application alone can delete through `session.storage`. `sandbox()` manages additional isolated
sandboxes and is separate from the shared default sandbox used by ordinary managed Tools.

`glob()`, `grep()`, `ls()`, `todo()`, `webSearch()`, and `webFetch()` are separate opt-ins. Duplicate
selections fail before session creation.

`subagents()` operates durable direct child sessions with explicit spawn, message, follow-up, wait,
list, interrupt and end actions. Children have independent journals/context but share the root's
default sandbox; use the separate `sandbox()` Tool when isolation is required.
