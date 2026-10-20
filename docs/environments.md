# Managed tools

Managed tools let an agent use an account-authorized service while your application disconnects. Call `await aex.tools()` for the deployed catalog. The deployment chooses operations, account grants, quotas and immutable resource profiles.

```js
import { ManagedTool } from "@aexhq/sdk";
const tools = [
  ManagedTool("session", { operations: ["inspect", "history"] }),
  ManagedTool("sandbox", {
    operations: ["create", "exec", "inspect", "terminate"],
    parameters: { profile: "YOUR_AUTHORIZED_PROFILE", lifetime_ms: 60000 },
  }),
];
```

Pass `tools` to `aex.sessions.create(...)`. The application chooses the profile and maximum lifetime; the model cannot change them. Aex reserves prepaid capacity before creating the session. The sandbox provider runs a fixed 1 CPU / 1 GiB allocation with the profile's image and network policy. Commands have bounded duration and output.

The managed `browser` provider offers `browse` actions through an isolated remote Chromium context. Its network origins are installed by the operator. This is separate from [tools in an attached user tab](client-browser.md).

The `session` provider inspects only the session to which it is bound. The model cannot supply another account or session ID. History uses opaque cursors and bounded pages. Installed `mcp` servers have fixed endpoints; endpoint selection is not model input.

[Application tools](http-tools.md) connect your own HTTPS service. Client functions declared with `Tool({run})` stay in the declaring process and require its callback connection.

## Cleanup and uncertain outcomes

Sandbox allocation and command dispatch are recorded before the external action. A lost acknowledgement never triggers another allocation or command. An uncertain allocation is reconciled by its provider tag. Removing a discovered resource does not establish its unknown start time; funding remains held until provider evidence is recorded.

Session deletion revokes bindings and waits for confirmed sandbox termination, remote-browser closure and application-result removal. Retry a 409 after cleanup. Terminal sandbox receipts settle cumulative observed duration once. Missing usage remains explicitly unknown.

There is no Environment preparation protocol, uploaded executable package or legacy-session importer. Historical resources and financial records remain available to operators through the retained pre-cutover pair.
