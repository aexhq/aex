---
title: Sessions and runs
---

# Sessions and runs

A session is the durable conversational and workspace boundary. Creating a
session does not run a prompt.

```ts
const session = await aex.sessions.create({ model: "openai/gpt-5" });
const { message, run } = await session.messages.send("Inspect the tests.");
const terminal = await run.result();
```

Message admission returns the accepted message and a durable run handle.
`get()`, `wait()`, and `result()` observe the canonical run resource.

Session lifecycle changes are explicit operations:

```ts
const persist = await session.persist({ include: ["reports/**"] });
await persist.result();

const deletion = await session.delete({ cascade: true });
await deletion.result();
```
