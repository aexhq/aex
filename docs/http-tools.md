# Application tools over HTTPS

Use an Application endpoint when tools must outlive a connected SDK process. Your service owns execution; Aex owns authorization, dispatch receipts and durable finish acknowledgements.

Declare the same JSON-schema tool descriptors on your backend and at session creation:

```js
import { Application, createApplicationHandler } from "@aexhq/sdk";
const descriptor = { name: "lookup", description: "Look up an item", input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] }, output_schema: { type: "string" } };
const handler = createApplicationHandler({
  credential: process.env.APPLICATION_TOKEN,
  tools: [{ ...descriptor, run: ({ id }) => `Item ${id}` }],
});
// Serve handler(request) from your web framework's HTTPS route.
const tools = [Application({
  endpoint: "https://your-service.example/tools",
  credential: process.env.APPLICATION_TOKEN,
  tools: [descriptor],
})];
```

Use a distinct random printable credential of at least 32 characters. Keep it on trusted backends. The endpoint must use HTTPS on port 443 without user information, query or fragment. Aex rejects private/reserved DNS destinations and redirects. Requests, responses, time and output are bounded.

A handler can return its result, report progress with `await context.progress(value)`, or call `await context.finish(value)` before its caller exits. A finish is acknowledged only after durable storage. Repeating the same finish is safe; changing an acknowledged result fails. The SDK validates tool input and declared output schemas.

Completion URLs are scoped opaque capabilities for the configured gateway origin. Use `gatewayOrigin` explicitly for a self-hosted gateway. They do not carry an account key. Do not forward them to unrelated services.

Aex records dispatch before contacting your service and never redispatches an uncertain call automatically. Inspect application state and committed history before deciding what to do. Session deletion invalidates completion authority and removes cached tool results after active work is cancelled.

Ordinary `Tool({run})` callbacks return JSON directly and use their invocation signal and progress callback; durable `context.finish` belongs to Application handlers.
