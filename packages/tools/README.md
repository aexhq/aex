# @aexhq/tools

Official, explicit tool selections for Aex sessions. Sessions have no tools unless the application
adds them at creation:

```ts
import { Aex } from "@aexhq/sdk";
import { computer, subagents } from "@aexhq/tools";

const aex = new Aex({ apiKey: "aex_sk_..." });
const session = await aex.sessions.create({
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    apiKey: "sk-ant-...",
  },
  tools: [computer(), subagents()],
});
```

`computer()` enables `bash`, `read`, `write`, `edit`, `glob`, `grep`, and `ls`. Each is also
exported separately for least-privilege sessions. `todo()`, `webSearch()`, and `webFetch()` are
separate opt-ins.

`subagents()` enables the stable `task` primitive. Each call runs one child in-process inside the
parent turn. Children start with isolated history, share the parent's model, workspace, and enabled
tools (except `todo`), and return their final report as the parent's tool result. Calls in one model
message can run in parallel. The engine caps nesting at three levels and child identities at 12 per
session, propagates cancellation, journals child decisions before dispatch, and never replays a
child after an interrupted process.
