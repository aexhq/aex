# Run tools in the user's browser tab

An attached tab can execute DOM callbacks while Aex retains the session. Keep account and model keys on your backend. First authorize the application user and issue scoped access for an exact frontend origin:

```js
const descriptor = { name: "selection", description: "Read selected text", input_schema: { type: "object", properties: {}, additionalProperties: false } };
const access = await aex.clients.grant({
  origin: "https://your-app.example",
  tools: [descriptor],
  session: {
    harness: "pi",
    config: { model: "openai/gpt-4.1-mini", tools: [] },
    model_credential: { api_key: process.env.OPENAI_API_KEY },
  },
});
// Return only access to the authorized frontend, with Cache-Control: no-store.
```

The frontend attaches inside an explicit user gesture:

```js
import { Aex, Tool } from "@aexhq/sdk";
const client = new Aex({ clientAccess: access });
const tools = [Tool({ ...descriptor, run: () => window.getSelection()?.toString() ?? "" })];
button.addEventListener("click", () => {
  client.attachTab({ create: { ...access.session, runtime_tools: tools } }).then(attached => {
    // Use attached.session; await attached.detach() when the user disconnects.
  });
});
```

Create within five minutes. The pending model credential stays in bounded server memory and is absent from the access response. A backend restart before creation requires a fresh grant. Access expires after one hour by default, with an explicit maximum of 24 hours. HTTPS origins are required except local HTTP development origins.

After a successful creation, read `await client.clients.composition()` to get the bound session ID. A later user gesture explicitly calls `client.attachTab({session:id,tools})`. An uncertain creation is not silently repeated. Scoped access cannot list other sessions, change the composition or manage an account.

Revoking the grant, its parent API key, or the account closes live streams and denies reconnects. Closing the tab disconnects callbacks; the session remains durable. There is no automatic idle disconnect or hidden callback replay. Use `detach()` or `client.close()` explicitly.

Use an [Application endpoint](http-tools.md) for backend work or a [managed browser](environments.md) for remote automation.
