# Run tools in your application backend

Use an Application environment when tools need your database or server dependencies.
Aex invokes one authenticated HTTPS request per tool call. Your backend can be a persistent
server or a serverless function; the request that submitted the turn can finish immediately.

Define the business function once:

```ts
// tools.ts
import { tool } from "@aexhq/sdk";
import { z } from "zod";
export const lookupOrder = tool({
  name: "lookup_order",
  description: "Look up an order",
  input: z.object({ id: z.string() }),
  run: async ({ id }, ctx) => {
    await ctx.finish({ id, status: "shipped" });
  },
});
```

Install `@aexhq/env-http` alongside the SDK and mount one handler in your existing POST
route. It accepts a standard Web Request and returns a Response; Hono can pass `c.req.raw`.

```ts
import { createToolHandler } from "@aexhq/env-http/handler";
import { lookupOrder } from "./tools.js";
export const POST = createToolHandler({
  tools: [lookupOrder()],
  authorize: async (request, invocation) => {
    await verifyApplicationCredential(request);
    await requireCurrentSessionOwner(invocation.sessionId);
  },
});
```

Those authorization functions belong to your application: verify the endpoint credential,
then resolve the session's user and current permissions from your database. Tool arguments
must not choose the authenticated user or tenant. Save the owner before the first turn.

Create the session on your backend:

```ts
import { Aex, brainEnv } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { lookupOrder } from "./tools.js";
const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const app = aex.environments.application({
  name: "orders",
  endpoint: "https://your-app.example/api/agent-tools",
  credential: process.env.AGENT_TOOLS_SECRET!,
  timeoutMs: 30_000,
});
const session = await aex.sessions.create({
  model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY! },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
  tools: [lookupOrder({ env: app })],
});
await saveSessionOwner(session.id, authenticatedUser);
const sequence = await session.submit("Look up order A-1001");
await aex.close();
```

Use a randomly generated endpoint credential of at least 32 characters. Aex admits the
endpoint, timeout, tool schemas and configured options during creation. No catalog lookup
or operator registration is required. Use the same configured tool options in the handler
and session; mismatches fail before business code. Placement changes through `{ env }`.
Later, use `aex.sessions.get(id)` and `session.outcome(sequence)` to read the committed outcome.

## Completion and request lifetime

`await ctx.finish(value)` waits for Brain's durable acknowledgment. Structured output,
optional model-facing content, async schema validation, configured options and
`ctx.emitResult(...)` use the ordinary Tool lifecycle. Application tools currently grant
completion services; model calls, arbitrary events and Environment control inside this
handler are unavailable.

The entire invocation must fit the endpoint's request budget. The default timeout is
30 seconds; the maximum is five minutes. Longer jobs belong in your durable job system:
finish with a job identifier and inspect it in a later tool call. Closing the submitting client
does not cancel a submitted turn.

Stored sessions retain their endpoint and sealed credential across restarts. Create a new
session when changing these declarations, and keep old endpoint credentials valid while
retained sessions need them. Aex checks account and issuing-key access before every dispatch.
It connects only to public HTTPS endpoints on port 443, checks addresses at socket creation
and refuses redirects. The handler receives an invocation-scoped completion capability,
never Brain's private address or credential. Completion remains available for already
dispatched work during drain.

Lost responses do not repeat business operations. Cancellation cannot roll back a committed
mutation. Keep business operation keys and receipts, and inspect them when an acknowledgment
is lost.

## Existing HTTP sessions and self-hosting

The earlier `http()` / `httpTool()` factories, account catalogs and configured bindings
remain supported. Their v1 handler contract returns JSON directly. Application placements
use the v2 completion protocol; both are accepted by the same handler and bridge.

Operators run `environments/http.mjs` privately on loopback port 8084. Keep the existing
`http_environments.url` and `public_url`; `bindings` may be empty for declaration-based
sessions. Endpoint credentials travel through Brain's sealed storage. Aex stores the public
declaration and account ownership. Public callbacks are relayed only to the fixed private
bridge, which validates each invocation grant.
