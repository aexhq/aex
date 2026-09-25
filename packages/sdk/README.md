# Aex SDK

Run AI agents on [Aex](https://aex.dev) from JavaScript or TypeScript. Connect your model and
tools, send messages, and read saved conversations without operating a Brain server.

## Get started

Create a key in the [dashboard](https://aex.dev/dashboard). With Node.js 22 or newer, install:

```sh
npm install @aexhq/sdk@0.84.0 @aexhq/agentloop-pi@7.2.2 zod@4
```

Set `AEX_API_KEY` and `OPENAI_API_KEY` in your server environment. Save as `order.mjs` and
run `node order.mjs`:

```js
import { Aex, brainEnv, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

const lookupOrder = tool({
  name: "lookup_order",
  description: "Look up an order by id.",
  input: z.object({ id: z.string() }),
  run: ({ id }, ctx) => ctx.finish({ id, status: "shipped" }),
});

const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
try {
  const session = await aex.sessions.create({
    model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY },
    agentloop: pi({ env: brainEnv({ name: "brain" }) }),
    tools: [lookupOrder()],
  });
  try {
    await session.send("Look up order A-1001. Has it shipped?");
    console.log(JSON.stringify(await session.transcript(), null, 2));
    console.log("Session:", session.id);
  } finally {
    await session.end();
  }
} finally {
  await aex.close();
}
```

The transcript includes an order lookup and the agent's answer. The lookup function runs in your
process; keep it connected while the agent needs that tool. The [quickstart](https://aex.dev/docs)
explains setup and where other tools run.

## Sessions and tools

The SDK uses Brain's session API and extension helpers. You can import `tool`, `agentloop`,
`brainEnv`, `hostEnv`, `component` and `environment` from `@aexhq/sdk`.

- [Sessions](https://aex.dev/brain/docs/concepts/sessions): send, submit, stream, reconnect and stop.
- [Tools](https://aex.dev/brain/docs/guides/write-a-tool): application functions and packaged tools.
- [Structured output](#structured-output): validated JSON answers.
- [Managed environments](https://github.com/aexhq/aex/blob/main/docs/environments.md): hosted tools and workspaces.
- [Attachments](https://github.com/aexhq/aex/blob/main/docs/attachments.md): upload images and PDFs.

Use `ctx.finish(value)` to complete a tool. Use `aex.close()` in `finally` to close client
connections. Closing the client keeps stored sessions; tools in your process still require it.

## Structured output

Ask for a typed application answer on one send. Before ending the session, pass a Zod
schema alongside the message:

```ts
const answer = await session.send("Return the order status", {
  output: { type: z.object({ id: z.string(), status: z.string() }), maxRetries: 2 },
});
console.log(answer.status);
```

Aex adds schema instructions to the prompt, reads that completed turn's assistant output,
parses JSON and validates it locally. The result has the schema's inferred output type.
Ordinary sends still return session state. The
[executable example](../../examples/structured-output.mjs) shows a complete session;
the [hosted guide](https://aex.dev/docs#structured-output) describes the same contract.

`maxRetries` counts additional correction turns and defaults to two. Zero validates one
candidate. Invalid JSON or Zod issues produce a follow-up asking for a complete corrected
answer. Fences and surrounding prose fail parsing. Normal Zod semantics apply: ordinary
objects strip unknown properties, strict objects reject them, and defaults, transforms and
async refinements run locally. The prompt describes the input shape; the returned value is
the parsed output. Unrepresentable schemas fail before sending, and custom-validator
exceptions propagate without corrections.

Exhaustion throws `StructuredOutputError` with `attempts`, `lastOutput` and `issues`.
Provider, transport, failed-turn and unsupported-output errors do not trigger corrections.
The loop must emit an Agentloop-origin `output_emitted` assistant message, as Pi and Codex do.
Aex reads the last such message within the exact completed turn, including idempotent
receipts from earlier history. It does not scrape internal model calls or infer refusals
from arbitrary text. No provider response format is set or cleared; avoid conflicting
session-level `responseFormat` settings.

Each attempt is an ordinary durable turn and can invoke Tools. Asking the agent not to
repeat actions is not enforcement. The caller must remain alive; rejected answers stay in
history and streams, and local validation failure does not rewrite a completed server turn.
Use exclusive ownership of sends during a typed operation. Overlapping send/submit calls
on the same wrapper fail; other handles and processes need application coordination.
An optional top-level `signal` cancels the active turn, aborts event reads and stops further
corrections. Async validators finish before their result is checked for cancellation.

The initial turn uses the supplied `idempotencyKey`; corrections use distinct derived keys.
Re-entry reuses completed turns when prompts and validation feedback are identical. This
is not an atomic server operation; nondeterministic validation can conflict on replay.

For work submitted by a short-lived caller, configure the official Agentloop with
`output: { schema: z.toJSONSchema(answerType), maxCorrections: 2 }` and use `submit()`.
That separate extension policy validates inside the hosted turn and prohibits Tools during
correction. It supports JSON Schema, not arbitrary local Zod refinements or transforms.

### Migrating from Brain SDK typed sends

Brain SDK 0.34 removes typed sends and the corresponding types and errors. Aex SDK 0.84
owns them. Existing Aex `send(..., { output })` calls keep the same syntax and behavior.
Import `StructuredSendOptions` and `StructuredOutputError` from `@aexhq/sdk`.
Use Pi/Codex 7.2.2 and extension packages pinned to Brain 0.34 with this SDK.
Mixing exact Brain dependency versions creates different TypeScript extension brands.

An existing standalone Brain handle can use Aex's policy without changing servers:

```ts
import { AexSessionHandle } from "@aexhq/sdk";
const session = new AexSessionHandle(await brain.sessions.get(sessionId));
```

`Aex` and `AexSessionHandle` use composition, so they are not instances of the upstream
`Brain` and `SessionHandle` classes. Use `AexSessionHandle` for explicit product-handle
annotations. Neutral Brain classes and extension helpers are still re-exported with their
original identities; `withToken()` still returns a raw Brain client. Stored sessions,
server contracts, native model formats and Tool schemas are unchanged.

## Account and usage

```js
console.log(await aex.account.get());
console.log(await aex.account.usage());
console.log(await aex.account.modelUsage(sessionId));
console.log(await aex.environments.list());
```

Run these before closing the client. Workload keys can read account and usage information.
Creating keys, accepting prices, topups and refunds require an account login; use the
[dashboard](https://aex.dev/dashboard) or [CLI](https://www.npmjs.com/package/@aexhq/cli).

See the [billing guide](https://github.com/aexhq/aex/blob/main/docs/billing.md) for account API
methods and `maxCostMicroUsd`, the ceiling for a resource operation. Aex spending controls do
not cap your separate model-provider bill.

Inline Tools default to the process registering them. Published libraries use Brain's generated
bindings and the selected Environment's prepared runtime. `ctx.finish(value, { content: summary })`
retains the structured result while offering concise model text. `ctx.model({ messages })` runs an
independent request within the invocation's lifetime and the session's model authority; hosted
usage and spending controls include those requests. The Agentloop owns the shared conversation.

Connect short application functions through an authenticated HTTP route with
[HTTP tools](https://github.com/aexhq/aex/blob/main/docs/http-tools.md). Submit a turn, close the
request and read its committed outcome later. Desktop apps can use
[upload-only grants](https://github.com/aexhq/aex/blob/main/docs/attachments.md#upload-from-a-desktop-without-an-account-key)
without receiving an account credential.
