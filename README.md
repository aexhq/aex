<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="apps/site/public/brand/aex-icon-white.png">
    <img alt="Aex" src="apps/site/public/brand/aex-icon-black.png" width="88">
  </picture>
</p>

<h1 align="center">Aex</h1>

<p align="center"><b>The agent cloud platform.</b></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@aexhq/sdk"><img alt="npm" src="https://img.shields.io/npm/v/@aexhq/sdk.svg?color=9a3d16&label=%40aexhq%2Fsdk"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-9a3d16"></a>
  <a href="https://aex.dev/docs"><img alt="Docs" src="https://img.shields.io/badge/docs-aex.dev-9a3d16"></a>
</p>

Give your agent a session: a conversation it remembers, a Linux machine to work
on, and tools that touch real files. You bring the model key. Aex runs the rest.

> [!WARNING]
> Aex is in active prelaunch development. Not all documented capabilities are
> deployed or verified, and the SDK, CLI, APIs, and hosted service are not
> guaranteed to work. Expect breaking changes and interruptions; do not rely on
> Aex for production workloads yet.

## Install

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

const answer = (async () => {
  for await (const frame of aex.sessions.sessionMessagesStream({ sessionId: session.id })) {
    if (frame.kind === "committed" && frame.message?.role === "assistant") return frame.message;
  }
  throw new Error("the stream ended before the assistant answered");
})();

await aex.sessions.sessionMessageSend({
  sessionId: session.id,
  body: { text: "Read the repository and tell me which tests are failing." },
  idempotencyKey: crypto.randomUUID(),
});

console.log(await answer);

await aex.sessions.sessionTerminate({
  sessionId: session.id,
  body: {},
  operationId: newId("operation"),
});
```

## What you get

- **Your key, your model.** Pick from OpenAI, Anthropic, DeepSeek, xAI, Meta,
  Moonshot AI, and Alibaba. Your key is encrypted the moment it arrives, never
  handed back, and never shared with another session.
- **A session that remembers.** One conversation holds the whole history. Stop
  the current work, shut the machine down and keep the transcript, or delete
  everything for good.
- **A machine of its own.** Every session gets its own Linux sandbox. It starts
  warming up right away, sleeps when nothing is running, and wakes up where it
  left off.
- **Files it can read.** Give it images, PDFs, video, source trees, or notes.
  They show up at fixed paths on the machine, and a later upload cannot change
  a session that is already running.
- **Tools that do real work.** Shell commands, reading and writing files, and
  your own MCP servers.
- **Helpers when it needs them.** The model can start child agents and run
  independent work at the same time, and the transcript still reads in order.
- **Watch it work.** Follow the answer as it is written, along with tool
  activity, logs, and usage. Replay or download it all afterwards.
- **Pay for what you run.** Top up a balance and see where it went by session
  or model. Your model tokens are billed by your provider, not by us.
- **Open source.** Apache-2.0, with the API contract and the tests in the open.

## Not here yet

An honest list of what is missing today:

- [ ] Hosted tools such as web search and browser control
- [ ] A shared place to keep secrets, skills, or tool bundles
- [ ] Sending your telemetry to your own endpoint, or querying it here
- [ ] SDKs in languages other than TypeScript

## Learn more

- [Documentation](https://aex.dev/docs)
- [Contributing](CONTRIBUTING.md) and [Security](SECURITY.md)
- Questions or partnerships: support@aex.dev
