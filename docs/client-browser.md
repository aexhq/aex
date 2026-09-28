# Run tools in the user's browser tab

Use `clientBrowser({ name })` for tools that read or change the user's current page.
The SDK opens the command stream and returns tool results through the ordinary session API.
Keep account and model-provider keys on your backend.

Share the composition between your backend and frontend:

```ts
// composition.ts
import { clientBrowser, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";
const readSelection = tool({
  name: "read_selection",
  description: "Read the user's current text selection",
  input: z.object({}),
  run: (_, ctx) => ctx.finish(window.getSelection()?.toString() ?? ""),
});
export const composition = {
  agentloop: pi(),
  tools: [readSelection({ env: clientBrowser({ name: "editor" }) })],
};
export const model = { provider: "openai", name: "gpt-4.1-mini" };
```

The backend prepares the declaration without executing the browser function. In your
authenticated bootstrap route, authorize the application user before issuing access:

```ts
const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const access = await aex.clients.grant({
  origin: "https://your-app.example",
  session: {
    ...composition,
    model: { ...model, apiKey: process.env.OPENAI_API_KEY! },
  },
});
return Response.json(access, { headers: { "cache-control": "no-store" } });
```

Then create and use the session in the browser:

```ts
const access = await fetch("/api/agent-access", { method: "POST" }).then(r => r.json());
const aex = new Aex({ clientAccess: access });
const session = await aex.sessions.create({ ...composition, model });
await session.send("Summarize my selected text");
```

Scoped access authorizes one fixed composition, host and resulting session at the specified
origin. Repeating `sessions.create()` with that composition reattaches to the same session,
including after an Aex restart. It cannot list other sessions, change model selection,
admit other packages or manage the account. Provider keys never appear in the access response.

Create within five minutes of issuance. Pending credentials stay only in server memory until
creation; interruption before creation requires fresh backend authorization. An uncertain
creation is never silently repeated. Access expires after one hour by default; `expiresAt`
accepts Unix seconds up to 24 hours. Use `aex.clients.revoke(access.id)` on the backend to
revoke it earlier. Parent-key revocation and account suspension also apply.

The browser sends its actual Origin. HTTPS origins are supported, with HTTP localhost
allowed for development. The command stream connects directly to Aex; the bootstrap route
does not keep a serverless request open.

The command connection suspends after five idle seconds. The same live client reconnects
before `send()`, `submit()` or an Environment operation; history reads leave it asleep.
Use `connectionIdleTimeoutMs: 0` when other callers or future autonomous events must
reach the tab. A suspended tab has no remote wake-up channel, and access is checked again
on reconnect. Explicit `aex.close()` permanently disposes of the client.

Closing the tab removes its tools and may interrupt their work; the durable session remains.
Use an [Application environment](http-tools.md) for backend tools that must outlive the tab.
`hostEnv` remains available for connected processes; the `browser` extension controls an
automation browser. A frontend that only sends messages needs no browser tool environment.
