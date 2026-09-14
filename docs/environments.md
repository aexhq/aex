# Managed tool environments

Aex hosts Brain and extensions. A managed Modal profile runs fixed Python, Node or other commands
with their prepared dependencies and data. One session binding shares one temporary workspace;
different bindings have isolated files. Code and data are built into an immutable Modal image.
This API does not accept arbitrary shell commands, images, dependencies or provider credentials.

```ts
import { Aex, brainEnv, tool } from "@aexhq/sdk";
import { codex } from "@aexhq/agentloop-codex";
import { z } from "zod";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY!, maxCostMicroUsd: 100_000 });
try {
  const catalog = await aex.environments.list();
  const workspace = await aex.environments.modal({ name: "analysis", profile: "python-v1", lifetimeMs: 300_000 });
  const calculate = tool({ name: "calculate", description: "Calculate a result",
    input: z.object({ value: z.number() }),
    implementation: { type: "modal_command", name: "calculate", configuration: { context: "application-owned" } },
  });
  const session = await aex.sessions.create({
    agentloop: codex({ env: brainEnv({ name: "brain" }), output: { schema: { type: "object" }, maxCorrections: 2 } }),
    model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY! },
    tools: [calculate({ env: workspace })],
  }, { idempotencyKey: "create-analysis-once" });
  const receipt = await session.submit("Calculate a result for 42.", { idempotencyKey: "turn-once" });
  // Save session.id and receipt in your application's database, then poll or stream Events.
} finally { await aex.close(); }
```

Use an ID returned in **your account's catalog** and a command it publishes. The example profile
is illustrative. Profiles are explicitly granted to accounts. Aex replaces the catalog driver
with its private controller and a sealed credential; clients never receive that credential.
Tool configuration is application data bound at creation. It cannot change the profile's grants.

Commands receive `{input, configuration, invocation: {sessionId, environment, sequence}}` on stdin
and return one JSON value on stdout. `configuration` is `null` unless supplied in the implementation.
A run-scoped application callback can persist business results; keep database administrator,
model, Modal, Stripe and AWS credentials outside the sandbox. The command must validate its input
and scoped configuration. Brain's privileged invocation callback is not passed to the process.

## Lifetime and credits

First-version profiles use one physical core and 1 GiB of memory, with at most 300 seconds of
lifetime from session admission. The request's cost ceiling covers all selected environments.
Aex commits the session claim and its credit holds atomically before contacting Brain. Insufficient
funds, missing price acceptance, unavailable profiles or exhausted capacity deny creation first.
Preview accounts must explicitly enroll and receive credits before using managed compute.

Allocation is lazy until first use. Waiting for the model after allocation counts as sandbox time;
unallocated time is free. Provider timeouts are rounded down to whole seconds. Cancellation,
detach, teardown and expiry stop the entire resource, including other active commands. Its
workspace is temporary and a stopped or lost resource is never silently replaced.

The controller reports cumulative resource milliseconds and confirmed termination. Aex rates
them using the reservation's immutable pricebook, caps the charge at the admitted lifetime, and
releases the remainder only after terminal evidence. Repeated or delayed reports do not double
charge. Revoked keys and suspended accounts cannot dispatch more commands; settlement remains
available. An expired grant that never bound to a controller releases at zero cost. Bound but
unresolved resources keep their hold until confirmed stopped.

## Operator deployment

The generated config schema defines `environments`: private `url`, catalog `public_url`,
`app_name`, `active_per_account`, and a map of versioned profiles. Each profile contains explicit
`accounts` and a `specification` with immutable image, fixed commands, CPU/memory, maximum
lifetime, working directory, region, outbound domains and output limit. Publish a new profile ID
when its specification changes. An empty outbound list blocks sandbox network access.

Set a distinct `AEX_ENVIRONMENT_TOKEN` on Aex and the controller. The controller alone also receives
`MODAL_TOKEN_ID`, `MODAL_TOKEN_SECRET`, and `AEX_ENVIRONMENT_DATA_DIR` on retained disk. It runs
`node /opt/aex/environments/modal.mjs` from the Aex image, sharing a private network namespace with
Aex and Brain so operator and driver routes remain on literal loopback. It has its own user,
process/filesystem isolation and credential set. Production exposes only the Aex API.

The narrow controller credential authorizes only `/environments/config`, `/environments/authorize`
and `/environments/usage` on Aex's private operator listener. Profile definitions are read from Aex's
immutable records. Reconciliation observes original resources and delivers pending usage every
ten seconds; it never retries a possibly dispatched effect.

Retain the controller's SQLite directory in the paired backup. If a create reply was lost and
name lookup cannot establish the original resource, locate its ID in Modal. With the controller
stopped, run `modal.mjs recover SESSION ENVIRONMENT SANDBOX_ID` against that same directory and
configuration. It verifies original ownership tags, terminates that resource and settles usage.
Never invent a replacement ID, drop an unresolved hold or edit financial history.
