# `@aexhq/sdk`

TypeScript client for durable Aex sessions.

```ts
import { Aex, tool } from "@aexhq/sdk";
import { z } from "zod";

const lookup = tool(
  z.object({ id: z.string() }),
  async function lookup({ id }) {
    return database.customers.find(id);
  },
).client();

const aex = new Aex({
  apiKey: process.env.AEX_API_KEY!,
  client: { id: "customer-api" },
});
const model = {
  provider: "openai" as const,
  name: "gpt-5.4",
  apiKey: process.env.OPENAI_API_KEY!,
};
const session = await aex.sessions.create({ model, tools: [lookup] });

const result = await session.send("Answer the question.", {
  output: z.object({ answer: z.string() }),
});
```

`send()` returns text by default. Passing `output` returns a typed Promise. The client uses
`https://api.aex.dev` by default; the model `baseUrl` is independent of the Aex API origin.
`send()` resolves only the durable winning assistant message after provider recovery. The lower-
level `session.events()` stream is raw and attempt-aware: provisional frames have no durable cursor
and can later be superseded, so live renderers must key them by `attempt_id` and process
`model.attempt_superseded` instead of concatenating attempts.
The SDK always generates and reuses an `Idempotency-Key` for root/child creation and messages; pass
`idempotencyKey` explicitly when the application must recover the same operation across its own
process restart. Raw hosted REST calls must supply that header for those operations.

Hosted alpha has one managed-compute shape, `1gb` (0.5 vCPU and 1 GiB), so the SDK exposes no shape
selector. Child sessions and managed sandboxes inherit the root's physical seal.

Tools are immutable once the session is created. `.client()` keeps closures in this application;
`.server(import.meta.url)` bundles a module whose default export is the completed Tool value for the
session's shared managed computer. One customer-app socket is shared across sessions; call
`aex.close()` during graceful process shutdown to stop it and interrupt process-local work. A
closed `Aex` instance cannot create another session. The
default `.client()` registration
is derived from its contract. If one application intentionally has different closures with the
same name and schemas, give each a stable `.client({ registration: "..." })`; a live runner rejects
a registration collision instead of invoking the wrong closure. Sessions grant no execution
capabilities by default. Add only the official capabilities needed. Aex's reserved output protocol is
present but inert unless a particular `send({ output })` request arms it:

Hosted `.server()` bindings use distinct generation-lifetime unprivileged users, separate from the
ordinary shell, while sharing the workspace through a group. Declared environment secrets are not
written by Aex to the workspace, arguments, results, or logs. This blocks ordinary sibling reads,
not guest-root compromise or a Tool deliberately writing the value. Use `.client()` or an external
service for that stronger boundary; local mode is intentionally unsandboxed.

```ts
import { bash, edit, read, sandbox, storage, subagents, write } from "@aexhq/tools";

const session = await aex.sessions.create({
  model,
  tools: [bash(), read(), write(), edit(), storage(), sandbox(), subagents()],
  network: { outbound: "none" },
});
```

Omitting `tools` and passing `tools: []` are equivalent. Zod schemas must be representable as JSON
Schema; process-local refinements and transforms fail before model work starts. Temporary files are
available through `session.sandbox.files`; durable objects use `session.storage`. Large transfers
automatically bypass the Brain actor. Durable direct children use `session.children`.
Sandbox file operations honor guest permissions; they do not bypass a `.server()` binding's
deliberate mode-0600 files. The binding must explicitly export or relax those permissions.

Buffered `upload()` and `download()` are convenient for small objects. Large transfers can stay
O(1)-heap with `downloadStream()` and a replayable declared source:

```ts
import { createReadStream } from "node:fs";
import { Readable } from "node:stream";

await session.storage.upload("inputs/archive.tar", {
  bytes: stat.size,
  sha256: digest,
  stream: () => Readable.toWeb(createReadStream(filename)),
});
const body = await session.storage.downloadStream("inputs/archive.tar");
```

The same methods are available on `session.sandbox.files`; the SDK enforces upload length, ticket
ceilings, abort signals, and the downloaded object's exact metadata length while bytes bypass
Brain. Direct large sandbox transfers are a happy-path convenience: the SDK does not automatically
retry an ambiguous completion or recover a ticket after Brain restart or expiry. Inspect the
generation and path, then prepare a fresh transfer. Put bytes in `session.storage` and copy them to
or from the sandbox when the transfer itself must be recovery-safe.

`await session.end()` closes work but retains journal and storage. Both deletion modes first make
the same short, durable deletion-job request. `await session.delete()` then polls the status
resource for confirmed removal; `{ queue: true }` returns after acceptance. No HTTP request stays
open while sandbox and storage cleanup runs.
