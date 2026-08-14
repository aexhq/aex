# @aexhq/sdk

The TypeScript SDK for Aex, with no dependencies of its own.

```bash
npm install @aexhq/sdk
```

## Quickstart

```ts
import { Aex, newId } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_WORKSPACE_API_KEY! });
const session = await aex.sessions.sessionCreate({
  body: {
    provider: "openai",
    model: process.env.AEX_MODEL!,
    providerApiKey: process.env.AEX_PROVIDER_API_KEY!,
  },
  idempotencyKey: crypto.randomUUID(),
});

const committed = (async () => {
  for await (const frame of aex.sessions.sessionMessagesStream({
    sessionId: session.id,
  })) {
    if (frame.kind === "committed" && frame.message?.role === "assistant") {
      return frame.message;
    }
  }
  throw new Error("message stream ended before the assistant committed");
})();

await aex.sessions.sessionMessageSend({
  sessionId: session.id,
  body: { text: "Inspect the tests and summarize the failures." },
  idempotencyKey: crypto.randomUUID(),
});
console.log(await committed);

await aex.sessions.sessionTerminate({
  sessionId: session.id,
  body: {},
  operationId: newId("operation"),
});
```

Creating a session takes the exact provider and model you want plus your own
`providerApiKey`, which is write only. That key is encrypted for the session,
never returned, and never shared with another one. You can use OpenAI,
Anthropic, DeepSeek, xAI, Meta, Moonshot AI, and Alibaba.

## The client

Session and file calls use a workspace `apiKey`. Account, API key, and billing
calls use a `dashboardSession` from signing in with GitHub. One client can hold
both, and it only ever sends each one to the service it belongs to.

```ts
const aex = new Aex({
  apiKey: process.env.AEX_WORKSPACE_API_KEY!,
  dashboardSession: process.env.AEX_DASHBOARD_SESSION!,
});
```

The namespaces are `apiKeys`, `auth`, `billing`, `bootstrap`, `registry`,
`sessions`, and `uploads`. `aex.workspaceFiles` adds friendlier helpers for
working with files. `ROUTES` and `aex.execute(...)` give you the same generated
contract directly, so there is no second route table to keep in step.

## Sessions and machines

A session is the only thing you create. It holds the conversation, the work in
flight, the tools it has used, its helper agents, its mounted files, its
machine, and its telemetry.

Every session gets a Linux machine unless you turn it off. It starts warming up
as soon as the session is created and goes to sleep when nothing is running.
The next tool call waits for it and wakes it if needed. Set `sandbox.enabled`
to `false` to get no machine at all; tools that need one then return a clear
error the model can handle.

Cancel ends the current work and leaves the session idle. Terminate destroys
the machine but keeps the messages and telemetry. Delete removes everything the
session owns; files you uploaded to the workspace survive.

## Files, tools, and MCP

`aex.workspaceFiles` gives you `put`, `upload`, `get`, `list`, `download`, and
`delete`. Each name has one current value, which you can set from text or
bytes, an HTTPS link, or a direct upload. Creating a session pins the names and
paths it uses, so replacing a file later cannot change a session already
running.

Messages are text only. Upload images, PDFs, video, archives, or source, then
tell the model which path under `/workspace` to look at.

Out of the box the model gets Bash, file read, edit, and write, MCP,
`storage.persist`, and tools to start, wait for, and stop helper agents. Attach
MCP servers per session, either over HTTP or as a process on the machine. A
large tool result leaves a short preview in the transcript and keeps the full
bytes on the machine, and `storage.persist` saves a chosen file back to your
workspace. A session can start at most 12 helper agents, nested no more than
three deep.

`sessionMessageSend` takes an optional `responseFormat` for JSON Schema output.
Aex allows it only when the provider and model support it natively, and will
not pass prompting off as the real thing.

## Streaming, telemetry, and billing

`sessionMessagesStream` sends preview, gap, committed, reconcile, and heartbeat
frames. Committed messages keep text, tool calls, and tool results in the same
shape whichever provider you use.

Telemetry comes as a live stream, a replay, and a verified compressed download.
The billing methods cover your prepaid balance, saved card details, hosted card
setup and top up, a permanent record of transactions, and usage. Billing needs
`dashboardSession`; sessions and files need `apiKey`.

See the [Aex documentation](https://aex.dev/docs) for fuller examples.

## Status

Aex is in active prelaunch development. Expect breaking changes, and do not
rely on the hosted service for production workloads yet.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
