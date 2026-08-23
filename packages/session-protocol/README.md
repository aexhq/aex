# `@aexhq/session-protocol`

Generated Brain session types and the low-level transport resources used by `@aexhq/sdk`.

Application code should use `@aexhq/sdk`. Environment and tool extension authors should use
`@aexhq/environment` and the SDK's `tool()` API. This package deliberately contains no tool
authoring, placement inference, or default environment.

The protocol exposes environments by logical name:

```ts
const environment = new SessionEnvironment(transport, sessionId, "workspace");
const status = await environment.status();
```

Durable session storage is independent of environment lifecycle. Its copy operations also require
the logical environment name and a generation fence.
