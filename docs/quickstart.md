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

Brain seals model capacity instead of guessing from a mutable model-name catalog. The conservative
default is 32,768 tokens; set `model.contextWindowTokens` when the selected model has a different
context window so admission and compaction use the intended immutable limit.

Hosted alpha uses one fixed managed-compute shape: `1gb` (0.5 vCPU and 1 GiB). The SDK therefore
has no shape selector; children and every managed sandbox inherit that seal.

## Where Tools run

Use one placement suffix for custom functions:

```ts
const inThisApp = tool(schema, async function inThisApp(input) {
  return serviceUsingThisProcessEnvironment(input);
}).client();

export default tool(schema, async function processFile(input, context) {
  return processInsideManagedComputer(input, context.workspace);
}).server(import.meta.url, { env: ["PROCESSOR_TOKEN"] });
```

- `.client()` executes in the application process connected to Aex. Its closures and environment
  remain there; the callback must enforce the application's own tenant authorization.
- `.server()` executes through the session's shared, lazily-created default managed computer.
  The completed Tool value must be the module's default export in the MVP, as in the example.
- Placement, Tool contracts and network policy are immutable after session creation. A model cannot
  choose or change them.

In hosted managed compute, each `.server()` binding runs as its own generation-lifetime
unprivileged user; the ordinary shell uses another user and the workspace is shared through a
group. Aex injects declared environment secrets into that binding without writing them to the
workspace, process arguments, result, or logs. This prevents ordinary sibling bindings and shell
code from reading the environment directly. It does not protect against guest-root compromise or
a Tool intentionally copying a secret into the shared workspace or its result. Keep the stronger
boundary in `.client()` code or an external service when that distinction matters. Explicit local
mode is unsandboxed and does not provide the hosted UID boundary.

The default client registration is derived from the Tool contract. If the same Node application
intentionally uses different closures with identical names and schemas, assign each a stable unique
`.client({ registration: "customer-lookup-v2" })`; collisions fail before session creation.

If `network` is omitted, managed compute has no outbound network. `{ outbound: "public" }` enables
supported public destinations while directly blocking private, metadata and Aex infrastructure.
Allowlist policies can seal narrower hosts or CIDRs. This policy does not govern `.client()` code.
Managed allowlist egress uses the platform's HTTP CONNECT proxy; SOCKS is not part of the MVP.

## Temporary files and durable storage

The default sandbox is shared by the root session and its children. Its files are temporary:

```ts
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

The Aex SDK always connects to the hosted production composition at `https://api.aex.dev` unless a
different `baseUrl` is supplied for development. `docker compose up --build` starts the Aex-owned
composition with `BRAIN_MODE=local`, reusable SQLite/session storage, the same fixed official Tool
policy, fake payments, and unsandboxed Tool execution inside the Brain container. `.client()` Tools
still run in the connected Node application; `.server()` Tools use the explicit local host Hand.
Aex never silently falls back from hosted execution to local execution. Local storage supports
restart-safe inline objects up to 1 MiB; large presigned transfers are hosted-only in the MVP. Call
`aex.close()` during graceful application shutdown to close the shared customer-Hand connection;
the closed client cannot create another session.

[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Hosted policy overlay](../contracts/hosted/brain-session.overlay.yaml) ·
[Control API](../contracts/control/v1/openapi.yaml)
