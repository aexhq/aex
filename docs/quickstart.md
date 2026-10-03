# Run your first agent on Aex

Aex hosts the agent session while your application supplies a model key and tools. This example
asks an agent to look up an order and prints its answer.

## 1. Get your keys

You need Node.js 22 or newer and an OpenAI API key. Sign in at
[aex.dev/dashboard](https://aex.dev/dashboard) and create an API key. Save it when it appears;
the secret is shown only once. Set both keys in your terminal:

```sh
export AEX_API_KEY="your-aex-key"
export OPENAI_API_KEY="your-openai-key"
```

In PowerShell use `$env:AEX_API_KEY = "your-aex-key"` and
`$env:OPENAI_API_KEY = "your-openai-key"`. Keep keys on your server and out of source control.
Your model provider bills model calls separately from Aex hosting.

## 2. Install

```sh
mkdir aex-example
cd aex-example
npm init -y
npm install @aexhq/sdk@0.88.0 @aexhq/agentloop-pi@9.0.0 zod@4.4.3
```

## 3. Create a session

Save this as `order.mjs`, or use the [complete example](../examples/order.mjs). The same API works in TypeScript.

```js
import { Aex, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

const lookupOrder = tool({
  name: "lookup_order",
  description: "Look up an order by id.",
  input: z.object({ id: z.string() }),
  run: ({ id }, ctx) => ctx.finish({ id, status: "shipped" }),
});

const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
aex.sessions.create({
  model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY },
  agentloop: pi(),
  tools: [lookupOrder()],
}).then(async session => {
  const after = session.state.lastSequence;
  await session.send("Look up order A-1001. Has it shipped?");
  for await (const event of session.events(after)) {
    if (event.type === "turn_failed") throw new Error(JSON.stringify(event.data));
  }
  console.log(JSON.stringify(await session.transcript(), null, 2));
  console.log("Session:", session.id);
}).catch(error => {
  console.error(error);
  process.exitCode = 1;
});
```

## 4. Run it

```sh
node order.mjs
```

The transcript includes the `lookup_order` result and an answer that order A-1001 has shipped.
The exact wording depends on the model. Replace the sample tool's body with your own lookup.
The client releases its tool connection after five idle seconds, letting this script exit.
Add more `session.send(...)` calls in the callback to continue the conversation. A later send
through the same live client reconnects automatically.

The example keeps the session and its history. Save its ID, then use
`await aex.sessions.get(sessionId)` and `session.transcript()` to read it later. Use
`session.end()` to finish the conversation and `session.delete()` to remove its history.
Explicit `aex.close()` permanently disposes of the client. Set `connectionIdleTimeoutMs: 0`
on `new Aex(...)` when its tools must remain available to other callers or future autonomous events.
Event subscriptions stay open until you stop them.

## Where code runs

| Code | Where it runs | What you need |
| --- | --- | --- |
| Pi or another packaged loop | Aex | Install the extension package |
| A tool with a `run` function | Your application (`hostEnv`) | Keep its process connected |
| A packaged tool | Aex (`brainEnv`) | Build a compatible extension; no file, network or server-secret access |
| A managed tool | A profile granted to your account | Accepted prices, credits and an operation cost ceiling |

For a request that ends before the work does, use `session.submit()` with tools hosted outside
that request. Calling `aex.close()` leaves hosted work running but disconnects application tools.
See [managed environments](environments.md) for available profiles and lifecycle.

## Next steps

- [Write a tool](https://aex.dev/brain/docs/guides/write-a-tool). Import shared helpers from
  `@aexhq/sdk` when using Aex; their contracts are the same as Brain's.
- [Choose a model](https://aex.dev/brain/docs/concepts/model) with your provider's key.
- [Send images and PDFs](attachments.md).
- [Get structured output](https://aex.dev/docs#structured-output).
- [Inspect credits and usage](billing.md), or use the [CLI](../packages/cli/README.md).

Aex is in early preview. Maintenance can interrupt work; saved history remains available,
and uncertain actions are not automatically retried.

The SDK prepares hosted code during session setup. To do that earlier and reuse it across
sessions, call `const loop = await aex.prepare(pi())` during startup, then use `agentloop: loop`.
See [Environment preparation](https://github.com/aexhq/aex/blob/main/docs/environments.md#prepare-hosted-code).
