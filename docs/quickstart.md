# TypeScript quickstart

Create an Aex API key in the [dashboard](https://aex.dev/dashboard), then install the SDK and the
four ordinary component packages:

```sh
npm install @aexhq/sdk @aexhq/tools @aexhq/env-aws-microvm @aexhq/loop-pi @aexhq/model-openai zod
```

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { openai } from "@aexhq/model-openai";
import { bash, read, write } from "@aexhq/tools";
import { z } from "zod";

const aex = new Aex({
  apiKey: process.env.AEX_API_KEY!,
});
const session = await aex.sessions.create({
  model: {
    component: openai(),
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
  },
  agentloop: pi({ instructions: "Work carefully and verify changes." }),
  environments: { workspace: awsMicrovm() },
  tools: [bash(), read(), write()],
  network: { outbound: "none" },
});

const result = await session.send("Review the customer record.", {
  output: z.object({ summary: z.string(), nextSteps: z.array(z.string()) }),
});

console.log(result.summary);
```

`session.send()` returns text by default. Passing `output` returns inferred data after runtime
validation. Call `send()` again to continue the same durable session.
The SDK generates a stable retry identity for every root or child creation/message. If the
application needs to retry across its own restart, pass an explicit `idempotencyKey`; raw hosted
REST calls must provide the 1-to-128-byte `Idempotency-Key` header for these operations.

Hosted alpha uses one fixed managed-compute shape: `1gb` (0.5 vCPU and 1 GiB). The SDK therefore
has no shape selector; children and every managed sandbox inherit that seal.

## Where Tools run

Hosted Tools are imported component values and run in their declared Environment. Application
callbacks use `tool()` with one `app()` Environment; their source and captured state remain in your
process:

```ts
import { tool } from "@aexhq/sdk";
import { app } from "@aexhq/env-app";

const lookup = tool(schema, async function lookup(input) {
  return serviceUsingThisProcessEnvironment(input);
});

await aex.sessions.create({
  model,
  agentloop,
  environments: { application: app({ id: "customer-app" }) },
  tools: [lookup],
});
```

Placement, Tool contracts and network policy are immutable after session creation. A model cannot
choose or widen them. If `network` is omitted, managed compute has no outbound network.

## Temporary files and durable storage

Every Environment is addressed by the name the session declared it under; `session.environments`
lists them and `session.environment(name)` selects one. `session.sandbox` is shorthand for the one
Environment a session declared. The sandbox is shared by the root session and its children, and its
files are temporary:

```ts
// created with environments: { workspace: awsMicrovm() }
const state = await session.sandbox.create();
if (!state.generation) throw new Error("sandbox has no live generation");
const entries = await session.sandbox.files.list("/workspace", {
  generation: state.generation,
});
const report = await session.sandbox.files.download("/workspace/out/report.pdf", {
  generation: state.generation,
});
```

A never-created sandbox returns `409 sandbox_not_materialized`; an expired or released generation
returns `410 sandbox_gone`. Reads never restore an old filesystem.
The file API honors guest Unix permissions. It does not bypass a `.server()` binding's deliberate
mode-0600 file, so the binding must explicitly export or relax permissions before another binding,
the ordinary shell, or `session.sandbox.files` can read it.

Use session storage for objects that must survive sandbox loss:

```ts
await session.storage.upload("inputs/data.csv", input);
await session.storage.copyFromSandbox({
  path: "/workspace/out/report.pdf",
  key: "outputs/report.pdf",
  sandboxGeneration: state.generation,
});
const objects = await session.storage.list({ prefix: "outputs/" });
```

There is no automatic workspace checkpoint or sync. Copying between temporary files and durable
storage is always explicit. Small payloads use bounded inline requests; larger uploads and downloads
automatically use short-lived scoped transfers so file bytes bypass the Brain session actor.
The hosted MVP limits one object to 512 MiB and visible plus reserved session storage to 10 GiB.
Only one large upload may be outstanding for a session; an abandoned unpublished upload expires and
its staging bytes are removed. Hosted accounts also have one authoritative 10 GiB total across all
root and child sessions. Reaching zero balance blocks new writes but does not silently delete
already-published storage.

For large objects, `session.storage.downloadStream()` and
`session.sandbox.files.downloadStream()` avoid buffering the whole result. `upload()` also accepts
`{ bytes, sha256, stream: () => ReadableStream<Uint8Array> }`; the SDK enforces upload length,
ticket ceilings, abort signals, and downloaded object length without an extra full-size copy.
Direct large sandbox transfers are intentionally happy-path only. Brain restart, ticket expiry, or
an ambiguous completion produces an honest error instead of an automatic replay; inspect the file
and generation, then call `upload()` or `downloadStream()` again to prepare a fresh transfer. Use
durable storage plus the explicit copy methods when transfer recovery matters.

## Exact billing values

Billing amounts such as `balance.microusd`, every `*_microusd` rate or charge, cumulative
`running_ms`, and `session_storage_byte_milliseconds` are canonical decimal strings. They remain exact
beyond JavaScript's safe-integer range. Use `BigInt` for arithmetic and do not coerce them through
`Number`:

```ts
const charged = BigInt(usage.total_microusd);
const byteMilliseconds = BigInt(usage.sessions[0].session_storage_byte_milliseconds);
```

Current byte gauges, per-operation cent amounts, and bounded query counts remain JSON numbers
because their public maxima are below `Number.MAX_SAFE_INTEGER`.

The storage charge is reproducible with integer arithmetic:

```ts
const rate = BigInt(usage.rates.session_storage_gb_month_microusd);
const denominator = 1_000_000_000n * BigInt(usage.rates.month_hours) * 3_600_000n;
const storageMicrousd = (byteMilliseconds * rate) / denominator;
```

The byte-millisecond quantity is the durable closed integral plus the derived open interval through
that response's `metered_to`. A delayed durable transition can replace the open estimate upward or
downward on a later absolute usage response; the ledger overwrites that session's prior estimate
instead of incrementing it.

## Lifecycle

`await session.end()` crosses a short idempotent boundary and returns the ordinary session in
`ending`; strong reads eventually show `ended` after the recursively fenced tree has stopped.
Both deletion modes make the same short, durable request. `await session.delete()` then polls the
deletion status for confirmed removal. Use `await session.delete({ queue: true })` when durable
acceptance is sufficient and you will observe completion separately. Ending a session is logical
and retains its journal and storage; deletion is destructive. Hosted alpha accounts may retain 100
roots at once, independent of the smaller concurrent-root limit, so delete finished sessions you no
longer need.

The Aex SDK connects to `https://api.aex.dev` unless a different `baseUrl` is supplied. Call
`aex.close()` during graceful application shutdown to close the shared customer-Environment
connection; the closed client cannot create another session.

[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Hosted policy overlay](../contracts/hosted/brain-session.overlay.yaml) ·
[Control API](../contracts/control/v1/openapi.yaml)
