# AEX beta quickstart

Create an AEX API key in the dashboard, then install the SDK and Zod:

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

const result = await session.output(
  z.object({
    summary: z.string(),
    nextSteps: z.array(z.string()),
  }),
  "Review this repository and tell me what to do next.",
);

console.log(result.summary);
```

`session.output()` returns a normal typed Promise. AEX keeps the schema and any repair attempt out
of the session conversation, validates the result, and rejects with a typed error if it cannot
produce the requested shape. The SDK uses `https://api.aex.dev` by default.

Use `session.send()` when you want ordinary text, and call either method again to continue the same
session.

Reference: [session API](../contracts/session/v1/openapi.yaml) ·
[control API](../contracts/control/v1/openapi.yaml) · [refund runbook](operator-refunds.md)
