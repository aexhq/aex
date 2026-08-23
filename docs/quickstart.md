# TypeScript quickstart

Create an Aex API key in the [dashboard](https://aex.dev/dashboard), then install the SDK and the
extensions used by the session:

```sh
npm install @aexhq/sdk @aexhq/env-app @aexhq/env-aws-microvm @aexhq/loop-pi @aexhq/tools zod
```

```ts
import { app } from "@aexhq/env-app";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { Aex, tool } from "@aexhq/sdk";
import { bash, read, write } from "@aexhq/tools";
import { z } from "zod";

const lookupCustomer = tool(
  z.object({ id: z.string() }),
  async function lookupCustomer({ id }) {
    return database.customers.find(id);
  },
)
  .describe("Look up a customer in this application.")
  .returns(z.object({ id: z.string(), status: z.string() }));

const application = app({ id: "customer-api" });
const workspace = awsMicrovm();
const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
    contextWindowTokens: 200_000,
  },
  loop: pi({ instructions: "Verify work before returning it." }),
  environments: { application, workspace },
  tools: [lookupCustomer, bash(), read(), write()],
  network: { outbound: "none" },
});

const result = await session.send("Review the customer record.", {
  output: z.object({ summary: z.string(), nextSteps: z.array(z.string()) }),
});

console.log(result.summary);
```

`session.send()` returns text by default. Passing `output` returns inferred data after runtime
validation. Call `send()` again to continue the same durable session.

## Loops, tools, and environments

A session has no default loop or environment:

- `loop` is a required imported brain extension such as `pi()` or `codex()`.
- `environments` assigns stable logical names to opaque environment references.
- each selected tool is bound to one compatible environment.

The SDK auto-binds an unbound tool only when exactly one declared environment is compatible.
Otherwise session creation fails before model work. Use an opaque reference—not a string—to make
an ambiguous binding explicit:

```ts
tools: [lookupCustomer.bind(application), bash().bind(workspace)]
```

The callback tool above has no prepared computer artifact, so it matches `app()`. Official computer
tools carry a prepared artifact and match `awsMicrovm()`. A model cannot select or change placement.

## Building a computer tool

A computer environment does not need Node or Python preinstalled. A prepared tool carries its own
runtime and immutable dependencies. Define the tool normally and use `setup()` for one-time,
environment-local preparation:

```ts
export default tool(
  z.object({ path: z.string() }),
  async function inspectFile({ path }) {
    return inspect(path);
  },
).setup(async function prepareInspector() {
  await prepareIndex();
});
```

Declare the entry in `package.json`, then run `aex tools build`. The CLI bundles the handler, setup
entrypoint, Node runtime, and dependencies into content-addressed layers. Tool code does not pass
`import.meta.url` and does not name an environment provider.

## Environment files and durable storage

Environment handles are contributed by their extensions and retain their reference type:

```ts
const runtime = session.environment(workspace);
const status = await runtime.status();
await runtime.files.upload("input.txt", inputBytes);
const bytes = await runtime.files.read("output.txt");
```

Environment files are temporary and generation-fenced. Durable session storage is independent of
every environment lifecycle:

```ts
if (typeof status.generation !== "string") throw new Error("environment is not materialized");

await session.storage.upload("inputs/data.csv", inputBytes);
await session.storage.copyFromEnvironment(workspace, {
  path: "/workspace/output.txt",
  key: "outputs/output.txt",
  generation: status.generation,
});
const objects = await session.storage.list({ prefix: "outputs/" });
```

There is no automatic workspace checkpoint or sync. Small storage payloads use bounded inline
requests; larger uploads and downloads use short-lived scoped transfers. For O(1)-heap transfers,
use `downloadStream()` and the replayable streaming upload source accepted by `upload()`.

## Recovery and lifecycle

The SDK generates an idempotency key for root and child creation and messages. Pass an explicit
`idempotencyKey` when an operation may be retried across an application restart.

`await session.end()` stops the session while retaining its journal and storage. `delete()` is
destructive; `delete({ queue: true })` returns after durable acceptance instead of polling for final
removal. Call `aex.close()` during application shutdown to close callback-environment connections.

The production API is `https://api.aex.dev`. Supplying `baseUrl` selects an explicit development
composition; Aex never falls back from hosted execution to local host execution.

[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Hosted policy overlay](../contracts/hosted/brain-session.overlay.yaml) ·
[Control API](../contracts/control/v1/openapi.yaml)
