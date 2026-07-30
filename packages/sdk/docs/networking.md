---
title: Networking
---

# Networking

Network access is explicit in the session request:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  network: {
    hands: { mode: "public_internet" }
  }
});
```

Use `none` when tool execution must not reach the public internet. Registered
MCP servers are customer-trusted remote systems; store their header values as
workspace secrets and register only the secret names.
