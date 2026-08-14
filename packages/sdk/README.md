# @aexhq/sdk

The zero-dependency TypeScript SDK for the session-centered AEX public API.

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

Session creation takes an exact provider/model pair and a write-only
`providerApiKey`. The key is encrypted for that session, never returned, and
never reused by another session. Supported provider families are OpenAI,
Anthropic, DeepSeek, xAI, Meta, Moonshot AI, and Alibaba.

## Client surface

Regional session and file calls use a workspace `apiKey`. Central bootstrap,
API-key, and billing calls use a GitHub `dashboardSession`. A client may hold
both; the SDK sends each credential only to its owning API plane.

```ts
const aex = new Aex({
  apiKey: process.env.AEX_WORKSPACE_API_KEY!,
  dashboardSession: process.env.AEX_DASHBOARD_SESSION!,
});
```

Generated namespaces are `apiKeys`, `auth`, `billing`, `bootstrap`, `registry`,
`sessions`, and `uploads`. `aex.workspaceFiles` adds verified convenience
helpers for the latest-only file API. `ROUTES` and `aex.execute(...)` expose the
same generated contract without a second handwritten route table.

## Sessions and sandboxes

A session is the only public execution resource. It owns committed messages,
one active root message, durable tool effects, native subagents, frozen file
mounts, sandbox state, and telemetry. There is no public run resource.

The default sandbox starts preparing in the background when session creation
commits, then suspends when no tool call is waiting. A sandbox tool call waits
for the exact generation to become ready and resumes it when required. Set
`sandbox.enabled` to `false` to allocate no sandbox; sandbox-only tools then
return a structured error the model can handle.

Cancel stops active work and returns the session to idle. Terminate destroys
sandbox compute while retaining messages and telemetry. Delete irreversibly
removes session-owned content; independent workspace files remain.

## Files, tools, and MCP

`aex.workspaceFiles` provides `put`, `upload`, `get`, `list`, `download`, and
`delete`. Each logical name has only one current value. Inputs may be inline
text or bytes, an HTTPS URL, or a direct multipart upload. Session creation
freezes selected names and mount paths, so later overwrites do not change an
existing session.

Messages are text-only. Upload arbitrary images, PDFs, video, archives, or
source and tell the model which path under `/workspace` to inspect.

The built-in tool surface is Bash, file read/edit/write, MCP,
`storage.persist`, and native create/wait/stop subagent tools. Configure remote
Streamable HTTP or sandbox-process MCP per session. Large tool results keep a
bounded preview and the full bytes at a sandbox path; `storage.persist`
publishes a chosen sandbox file as the latest workspace value. Native
subagents are bounded to 12 child identities per session lifetime and depth 3.

`sessionMessageSend` accepts optional native JSON Schema output through
`responseFormat`. Strict structured output is admitted only for a qualified
provider/model capability; AEX does not present prompt emulation as strict.

## Streaming, telemetry, and billing

`sessionMessagesStream` emits bounded preview, gap, committed, reconcile, and
heartbeat frames. Committed messages preserve provider-neutral text, tool-call,
and tool-result parts.

Session telemetry is available as a live stream, retained replay, and verified
compressed download. Billing resources expose prepaid balance, saved-card
display metadata, hosted card setup and top-up, immutable transactions, and
usage. Billing methods require `dashboardSession`; session and file methods
require `apiKey`.

See the [AEX documentation](https://aex.dev/docs) for complete examples.

## Status

AEX is in active prelaunch development. Expect breaking changes and do not rely
on the hosted service for production workloads yet.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
