# Aex alpha quickstart

Create an Aex API key in the dashboard, then install the SDK and Zod:

```sh
npm install @aexhq/sdk zod
```

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

const result = await session.send(
  "Give me a concise launch plan for a small production AI application.",
  {
    output: z.object({
      summary: z.string(),
      nextSteps: z.array(z.string()),
    }),
  },
);

console.log(result.summary);
```

`session.send(input, { output: schema })` returns a normal typed Promise. The schema is scoped to
this send, Aex validates submissions in its trusted control plane, and the Promise rejects with a
typed error if the agent cannot produce the requested shape. The SDK uses `https://api.aex.dev` by
default.

Sessions start with no model tools. Install `@aexhq/tools` and pass the exact capabilities the
application wants to grant, such as `bash()`, `read()`, `write()`, `edit()`, or `subagents()`.
Omitting `tools` and passing `tools: []` are equivalent.

Omit `output` when you want ordinary text, and call `session.send()` again to continue the same
session.

Reference: [session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[control API](../contracts/control/v1/openapi.yaml) · [refund runbook](operator-refunds.md)
