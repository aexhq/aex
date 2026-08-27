# TypeScript quickstart

Install the hosted SDK and the extensions your session uses:

```sh
npm install @aexhq/sdk @aexhq/loop-pi @aexhq/tools @aexhq/env-aws-microvm
```

```ts
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { Aex } from "@aexhq/sdk";
import { packageUrl as piPackage } from "@aexhq/loop-pi";
import { definitions } from "@aexhq/tools";
import { awsMicrovm } from "@aexhq/env-aws-microvm";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const admission = await aex.brain.admitAgentloop(
  await readFile(piPackage),
  randomUUID(),
);
const environment = awsMicrovm({
  id: "workspace",
  lifecyclePolicy: "session",
});
const selected = [definitions.bash, definitions.read, definitions.write];

const session = await aex.sessions.create(
  {
    agentloop_digest: admission.digest,
    model: {
      binding_id: "vercel-ai-gateway",
      model: "openai/gpt-5.4",
    },
    presentation: {
      system: "Work carefully and verify changes.",
      tools: selected.map((tool) => tool.definition),
    },
    environments: [environment],
    tool_bindings: selected.map((tool) => ({
      name: tool.definition.name,
      environment_id: environment.environment_id,
      remote_tool_id: tool.remoteToolId,
      grant: {},
    })),
  },
  { idempotencyKey: randomUUID() },
);

await session.send("Inspect the workspace.", {
  idempotencyKey: randomUUID(),
});

for await (const event of session.events()) {
  console.log(event.event_type, event.data);
}
```

Agentloop packages contain policy only. They cannot open sockets, read secrets, call Tools, or use
clocks directly. Brain presents observations to the Component, executes its returned decision, and
activates it again with the result.

Tool definitions are part of the stable model presentation. Their implementation runs in the bound
Environment, never in Brain. An Environment adapter may manage a remote MicroVM, browser, sandbox,
or user process through the same setup/attach/execute/cancel/detach/teardown contract.

Every mutating SDK method accepts an idempotency key. Reuse the same key only for a byte-equivalent
logical request. Session events are finite cursor reads; reconnect from the last sequence to consume
the durable journal without relying on live telemetry.
