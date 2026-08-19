# Aex product shape: session, tools, output

Status: accepted; implementation in progress
Date: 19 August 2026
Scope: public product, API, SDK, CLI, dashboard, docs, pricing, and launch sequence

## Outcome

Aex should aggressively minimise its developer vocabulary.

The developer creates a session, gives it tools, sends it work, and optionally asks for a typed
output. Every session gets a computer automatically. Durable remote objects are assets.

There is no public Run resource or Run terminology.

```text
application
    |
    +-- session ---- context, history, agent, computer
          |
          +-- tools ----- capabilities selected in code
          +-- assets ---- durable remote objects granted to the session
          +-- output() -- a typed Promise over the session's answer
```

The public learn set is three nouns and one method:

| Concept | Meaning |
| --- | --- |
| Session | The durable agent context. It includes its history and an automatically managed computer. |
| Tool | A typed capability the developer imports or writes. |
| Asset | A private remote object that can outlive a session. |
| `session.output(schema)` | Ask Aex to commit and return a response conforming to the schema. |

`hand`, microVM, shape, region, workspace, artifact, turn, operation, journal sequence, provider
response format, and output repair are implementation terms. Developers should not need them to
use the product.

## Positioning

**The session backend for AI apps.**

> Start a session. Give it tools. Get back text, data, or files.

Postgres is the centre of Supabase; the surrounding products make Postgres easier to use. The
session is the centre of Aex; tools, a computer, assets, typed output, events, and billing make a
session usable in production.

Aex is not a sandbox product. E2B, Modal, and Daytona expose machine lifecycle and resource
choices. Aex uses computers internally but exposes an agent session. Aex is also not a general
deployment platform: the customer's application can stay on Vercel or anywhere else and call Aex
for agent work.

## The SDK surface

`@aexhq/sdk` is the primary alpha SDK. It defaults to `https://api.aex.dev` and accepts a placeholder
API key directly in the quickstart. Raw HTTP remains in generated API reference, not onboarding.

### Normal text

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex({ apiKey: "aex_sk_..." });

const session = await aex.sessions.create({
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    apiKey: "sk-ant-...",
  },
});

const answer = await session.send("Explain this repository in two paragraphs.");
```

### Typed output from the current session

```ts
import { z } from "zod";

await session.send("Research the three strongest alternatives and compare them.");

const comparison = await session.output(
  z.object({
    recommendation: z.string(),
    alternatives: z.array(
      z.object({
        name: z.string(),
        strength: z.string(),
        weakness: z.string(),
      }),
    ),
  }),
);

// comparison is fully inferred and validated.
```

### Typed output with a new instruction

For the common one-call case, the second argument is an optional real user message:

```ts
const comparison = await session.output(
  z.object({
    recommendation: z.string(),
    confidence: z.number().min(0).max(1),
  }),
  "Research the available options and recommend one.",
);
```

The TypeScript signature is conceptually:

```ts
output<T>(schema: OutputSchema<T>, input?: SessionInput): Promise<T>
```

With no second argument, Aex produces a typed projection of the session's current state. With an
input, that input is the only new user message and the agent may use its tools before committing
the typed answer.

Do not add `.result(callback).error(callback)` as an Aex-specific control-flow language. Returning
a real `Promise<T>` is smaller and more conventional:

```ts
const value = await session.output(schema);

session.output(schema).then(onResult).catch(onError);
```

Errors reject the Promise with typed Aex errors. Progress and tool events stay on the session event
stream rather than being mixed into the result API.

### Files as output

Files remain assets rather than a second output protocol. The agent uses the official assets tool
to upload a computer file, then returns the asset reference in typed data:

```ts
const result = await session.output(
  z.object({
    summary: z.string(),
    reportAssetId: z.string(),
  }),
  "Prepare the report and save the Markdown file as an asset.",
);

const report = await aex.assets.get(result.reportAssetId);
```

A small `assetRef()` Zod helper can replace the string once the base asset contract is stable. It
does not justify a separate Result, FileOutput, or composite-output abstraction in the MVP.

## How `session.output(schema)` works

### The context rule

Output control data must not become conversation content.

- Do not append the schema as a normal user message.
- Do not permanently modify the session system prompt.
- Do not append validation failures or repair instructions to session history.
- Do not expose a final-output tool during the ordinary working phase unless a provider requires
  it.
- Persist only the real user input and the final validated assistant output.

The schema is operation metadata held by Aex. It is visible to the model only during a private
output-commit phase and is absent from later requests.

### Two phases

An output request has a work phase and a commit phase.

#### 1. Work phase

If the caller supplied an input, Aex appends it as a real user message. The agent then works with
its normal stable system prompt, normal session history, and normal tools. The output schema is not
inserted into the conversation or ordinary tool list.

If no input is supplied, this phase is skipped. Aex captures the current committed session
sequence as the source state for the output.

Only one mutating action is admitted to a session at a time. Later messages wait behind the output
request, so the Promise refers to an exact session state rather than a moving target.

#### 2. Commit phase

When the agent has finished its work, Aex makes one additional model step to commit the typed
answer.

Use the session's configured model and provider key for this step by default. Aex should not hide a
second formatter model or send session data to another provider merely to shape the response.

The provider adapter receives:

- the captured session context;
- the agent's candidate final answer, if it produced one;
- the requested schema as provider control data;
- a short transient instruction such as “commit the answer in the required format; preserve the
  facts and do not perform new work.”

That instruction is not a user message and is never journaled. Use the provider's developer or
system control channel only for this one request when a textual instruction is required.

The preferred implementation order is:

1. Provider-native structured output or constrained decoding.
2. A forced internal `aex_output` tool followed by original-schema validation.
3. A transient format instruction plus local parsing only for a provider that supports neither.

The commit phase has no ordinary tools. It formats and synthesises already available session data;
it does not continue the task. If the desired fields require research or an external action, that
requirement belongs in the real user instruction. Aex guarantees the shape, not the factual quality
of information the agent was never asked to gather.

This extra model step is intentional. Current tool-plus-structured-output systems also treat the
structured response as an additional step. It keeps the schema out of the agent's working context
and gives Aex one provider-independent contract.

### Schema compilation

For alpha, make Zod the documented TypeScript path because it matches the proposed API and has a
mature JSON Schema conversion. Internally, keep the adapter boundary compatible with Standard
Schema and Standard JSON Schema so ArkType and Valibot can be accepted later without changing
`session.output()`.

At call time Aex:

1. extracts the inferred TypeScript type in the SDK;
2. converts the schema to JSON Schema;
3. rejects unsupported or unrepresentable constructs before invoking a model;
4. normalises the schema for the chosen provider's supported subset;
5. retains the original Zod schema for final local validation.

Provider schema support is not identical, so Aex validates the returned value locally even when a
provider guarantees its reduced schema. The alpha SDK rejects process-local Zod refinements,
transforms, and other constructs the service cannot enforce before it admits model work. A future
client-validation handshake could support those safely; silently committing a value and only then
discovering that a JavaScript callback rejects it would violate the method's contract.

Do not silently coerce values. A string `"42"` is not a number unless the user's Zod schema itself
declares a transform or coercion.

### Validation and repair

Recommended policy:

1. Parse the provider result.
2. Validate it with the original schema.
3. If valid, commit it and resolve the Promise.
4. If invalid, perform at most one isolated repair attempt.
5. Validate again; if invalid, reject with `OutputValidationError`.

The repair attempt receives only the captured session state, the invalid candidate, the validation
issues, and the same constrained schema. It is not another agent turn. It cannot use tools, and its
prompt and response do not enter the durable conversation.

One attempt is a good boundary. Unlimited “please fix the JSON” loops increase cost and can turn a
missing fact into a fabricated one. Providers with reliable native constrained decoding should
rarely need the repair path; it primarily handles constraints removed during schema normalisation
or fallback adapters.

Do not automatically return the invalid candidate as partial success. The method promises `T`, so
it either resolves to `T` or rejects. The dashboard and internal diagnostics may retain the
candidate under the account's normal data controls.

### Commit to session history

On success, append one structured assistant content block to the session journal:

```json
{
  "type": "output",
  "schema_hash": "sha256:...",
  "value": {}
}
```

Future model requests render that block as compact canonical JSON so the agent knows what it
returned. The schema, provider control prompt, invalid candidates, and repair messages are not
included. Large outputs can be stored out of line and represented by a reference plus a compact
summary.

The Promise resolves to the validated `value` directly. Session events can expose
`output.started`, `output.completed`, and `output.failed` for observability, but SDK users do not
need these events to obtain the result.

### Error model

Keep the errors small and actionable:

| Error | Meaning |
| --- | --- |
| `OutputSchemaError` | The schema cannot be represented safely for this output path. No model call was made. |
| `OutputRefusalError` | The provider refused the requested content. |
| `OutputValidationError` | The provider and one bounded repair attempt did not satisfy the original schema. Includes validation issues. |
| `SessionError` | The underlying session, model, computer, or tool operation failed. |
| `AbortError` | The caller cancelled the output Promise. |

Every output request gets an internal idempotency identity and schema hash. Retrying a request after
a transport interruption returns the same committed value rather than asking the model again. The
identity may appear in errors and diagnostics, but it is not a product resource.

## Why not a system or user message?

A normal user message is semantically wrong: the application did not ask a new conversational
question merely by requesting a TypeScript type. It would also remain in history and influence
future work.

A permanent system-prompt addition is also wrong. It adds tokens to every model step, changes the
agent's behaviour while it is using tools, and can invalidate provider prompt caches whenever the
schema changes.

Provider-native structured-output configuration is the cleanest control plane. Current Claude
structured outputs use constrained decoding and may add an internal system instruction, but that
configuration can be scoped to one API request. Aex should scope it to the private commit phase so
the durable session prompt stays stable.

The forced terminal-tool fallback is the next-best option. It exposes exactly one schema-bearing
tool only during the commit request and terminates on its successful call, following the useful
part of Pi's current structured-output example.

## Sessions

A session is the only execution resource the developer manages.

The public session supports:

```text
create
get / list
send
output
events
cancel current work
delete
```

There is no separate lifecycle object for each message or output. Aex still records internal
operation identities, terminal states, usage, and event boundaries for correctness, billing, and
debugging. The SDK and dashboard present them as activity inside the session.

An output does not automatically delete or close the session. After `output()` resolves, the
session is idle and can receive another message or produce another typed output.

## Tools

Tool selection is an array of imported values, never names:

```ts
const session = await aex.sessions.create({
  tools: [computer(), subagents(), webSearch(), assets(), lookupCustomer],
});
```

The accepted type is `Tool | Toolset`. A toolset may provide several namespaced operations, so an
array item does not have to correspond to one flat provider tool.

Custom tools use ordinary TypeScript and Zod:

```ts
const lookupCustomer = defineTool({
  name: "lookup_customer",
  description: "Look up a customer by email address.",
  input: z.object({ email: z.string().email() }),
  output: z.object({ id: z.string(), plan: z.string() }),
  async execute({ email }, context) {
    return database.customers.findByEmail(email, {
      signal: context.signal,
    });
  },
});
```

A hosted agent cannot serialise that function. In the alpha, callback tools execute in the
developer process attached through the SDK or `aex dev`:

1. Aex commits a uniquely identified tool call to the session journal.
2. The SDK validates the input and executes the callback.
3. The SDK submits an idempotent result.
4. Aex commits the result before continuing the agent.

The attachment has a lease. The SDK de-duplicates replayed calls. If the process disappears after
an ambiguous side effect, Aex marks that activity interrupted and never executes it again
automatically.

Tool signatures are fixed when the session is created for alpha. Hooks, approvals, call limits,
remote endpoints, managed tool deployment, dynamic tool changes, and MCP follow after launch.

An Aex tool package is an ordinary npm package exporting a `Tool` or `Toolset`. There is no
Aex-specific package manifest or marketplace in the MVP. `@aexhq/tools` initially exports explicit
computer tools, `subagents()`, `todo()`, `webSearch()`, and `webFetch()`. The durable-assets slice
will add `assets()` later.

## Computer and agent-allocated infrastructure

Every session gets one logical computer automatically. The developer does not configure whether
it exists, region, CPU, RAM, machine shape, substrate, or suspend behaviour. It contributes no
model tools by default: every capability is an explicit imported value. `computer()` enables the
standard computer toolset, while individual helpers such as `read()` and `write()` support a
smaller grant. Search, subagents, asset transfer, browser work, and customer integrations are also
explicit additions.

The contract promises that working files survive normal continuation during the session. Aex may
stop, restore, or replace the underlying machine. Process memory and machine identity are not
durable promises. Deleting the session deletes its working state.

After alpha, the official `sandbox()` tool lets the agent allocate additional isolated computers.
The developer supplies policy rather than infrastructure dimensions:

```ts
sandbox({
  policy: {
    maxComputers: 3,
    maxDuration: "20m",
    maxSpend: "2.00",
  },
});
```

The agent requests intent, such as a disposable test environment or browser-capable computer. Aex
chooses the allocation. This remains post-MVP but the policy-and-intent boundary is reserved now.

## Assets

An asset is private remote storage for something the application or agent needs beyond the
session.

Recommended alpha rules:

- account-owned and private by default;
- immutable, with a new ID for new content;
- explicitly granted to a session;
- survives session deletion;
- deleted only explicitly or through an optional expiry;
- transferred through short-lived, single-object signed URLs;
- visible to the agent only when attached to the session or created by it.

Do not add projects, buckets, folders, RLS policy language, or a workspace browser in the alpha.
The SDK provides upload, download, get, list, and delete. The `assets()` tool provides agent-facing
download-to-computer and upload-from-computer operations.

## Public API

Replace the prelaunch contract cleanly while retaining `/v1`:

```text
POST   /v1/sessions
GET    /v1/sessions
GET    /v1/sessions/{session_id}
DELETE /v1/sessions/{session_id}

POST   /v1/sessions/{session_id}/messages
POST   /v1/sessions/{session_id}/output
POST   /v1/sessions/{session_id}/cancel
GET    /v1/sessions/{session_id}/events

POST   /v1/assets
GET    /v1/assets
GET    /v1/assets/{asset_id}
DELETE /v1/assets/{asset_id}
POST   /v1/assets/{asset_id}/download
```

The output request carries the converted schema, schema hash, optional input, and idempotency key.
The API may return an accepted internal operation while the SDK follows session events, but the
SDK surface is one `Promise<T>`.

Keep sequence-number replay, `Last-Event-ID`, cancellation, fencing, and never-replay invariants.
Do not expose internal operation IDs as a collection developers must list or manage.

## SDK and CLI MVP

Ship:

- `@aexhq/sdk`: sessions, `send`, `output`, asset client, `defineTool`, events, typed errors;
- `@aexhq/tools`: computer tools, `subagents()`, `todo()`, `webSearch()`, `webFetch()`, and
  `assets()`;
- `@aexhq/cli`: binary name `aex`, able to load TypeScript tools.

Minimum CLI:

```text
aex login
aex dev
aex session list|get|delete
aex session send
aex session output --schema <file>
aex session events|cancel
aex asset put|get|list|delete
aex doctor
```

The CLI defaults to `https://api.aex.dev`. Alpha login can paste and store a dashboard-created API
key. Browser/device login follows. The current Rust CLI remains an internal diagnostic during the
migration; the public CLI should be Node-based so it can load the same TypeScript tools and Zod
schemas as the SDK.

TypeScript is the only launch SDK. Python follows with Pydantic after the contract settles.

## Pricing

Hide dimensions the user cannot choose:

| Meter | Recommended alpha price |
| --- | --- |
| Computer while active | $0.12 per hour, metered by the second |
| Retained session files and assets | $0.03 per GB-month |
| Managed web search | $0.003 per successful query |
| Model calls | The provider bills the developer directly |

Remove vCPU, RAM, shape, suspended-machine, and region rows. Release idle computers and restore
their working files instead of promising retained RAM. Internal infrastructure accounting can
remain detailed.

The private output-commit step is a model call through the developer's provider key. It should be
visible in session usage but does not create an Aex output fee.

## Dashboard and site

Dashboard navigation:

1. Overview: balance, top up, API key, and first-session form.
2. Sessions: state, recent activity, spend, conversation/events, current output, cancel/delete.
3. Assets: name, type, size, created time, download, delete.
4. Account: email, keys, billing, support.

There is no Runs page, tool registry, schema builder, workspace browser, infrastructure form,
region picker, or machine-size picker.

The public site stays prose-first and short:

```text
Aex

The session backend for AI apps.
Start a session. Give it tools. Get back text, data, or files.

[small session.output(schema) example]

Sessions
Context that survives more than one request.

Tools
Import capabilities or write an ordinary typed function.

Output
Await validated data. Keep schema mechanics out of your application.

Pricing
$0.12 active computer hour · $0.03/GB-month storage · $0.003 web search

Alpha
[waitlist]
```

Copy sweep:

- Display the product name as `Aex`; keep lowercase only in technical identifiers and domains.
- Use `Alpha` for the current invitation-only release.
- Reserve `Beta` for the first limited public release with regular signup.
- Remove `eu-west-1` from product, dashboard, status, docs, and metadata.
- Remove Run, hand, microVM, workspace, artifact, shape, suspended RAM, and same-machine promises.
- Replace raw HTTP onboarding with the SDK.
- Keep THINK SLOWLY LTD trading as Aex and `support@aex.dev`.
- Do not describe the alpha as UK-only; England and Wales governing law is not an access limit.

## MVP boundary

### Required for alpha

- Waitlist, invitation, email sign-in, dashboard onboarding, and API keys.
- Prepaid top-up, usage, balance, and unused-credit refund.
- TypeScript SDK and user-facing CLI.
- Durable sessions with `send`, `output`, events, cancel, and delete.
- Zod-based typed output with provider-native constraints, one bounded repair, and typed errors.
- One automatic default computer with hidden sizing.
- Private durable assets attached to sessions.
- Official web-search and assets toolsets.
- Callback custom tools while the SDK/CLI is attached.
- Minimal site, concise docs, legal pages, support, status, and alert verification.

### After alpha

- `sandbox()` for agent-created additional computers.
- Tool hooks, limits, approvals, remote tools, managed deployment, and dynamic tool manifests.
- Detached customer-tool execution.
- MCP and progressive tool discovery.
- Python SDK.
- Rich asset-reference schema helpers.
- Projects, teams, roles, buckets, custom retention, and fine-grained key scopes.
- Official agent skill at `aex/.agents/skills/aex`.
- Tool marketplace, multiple regions, compliance packages, and SLA tiers.

## Implementation order

### 0. Reconcile source branches

- Reconcile local `aex/main` with `origin/main` before contract edits.
- Reconcile local `site/main` with the Vercel and legal work on `origin/main`.
- Preserve unrelated user changes in deprecated `aex-backup`.

### 1. Contract first (`aex`)

- Remove public Run, hand, shape, workspace, files, and artifacts.
- Add session output request, structured output event/content block, asset, and tool manifest.
- Simplify the rate card.
- Regenerate Rust/TypeScript types and examples from schemas.
- Add conformance cases for native output, fallback tool output, repair, refusal, cancellation,
  replay, and idempotency.

### 2. Output vertical slice (`brain`, `aex-control`)

- Capture an exact session sequence for each output request.
- Add the clean work/commit split.
- Implement provider-native output adapters and the forced terminal-tool fallback.
- Add original-schema validation and one isolated repair.
- Commit only validated structured output to history.
- Resolve SDK requests from session events without creating a public resource.

### 3. Assets (`platform`, `aex-control`, `brain`, `hands`)

- Add account-owned asset metadata, signed upload/download, and session grants.
- Reuse the hand ABI's existing single-object presigned transfer model.
- Package download/upload as the official assets toolset.
- Remove the old public workspace-files and artifacts routes.

### 4. Tools (`brain`, `aex-control`)

- Compile imported definitions into a fixed session tool manifest.
- Add callback execution through journaled calls and lease-scoped results.
- Package managed web search and assets as imported toolsets.
- Keep MCP out of onboarding.

### 5. Hide compute (`aex`, `brain`, `platform`, `hands`)

- Make the default computer unconditional and remove public sizing.
- Select the alpha allocation through platform policy.
- Promise working-file continuation, not RAM or machine identity.
- Release idle machines and consolidate storage metering.

### 6. SDK and CLI (`aex`)

- Rebuild and publish `@aexhq/sdk`, `@aexhq/tools`, and `@aexhq/cli`.
- Implement Zod conversion, typed Promise results, errors, event replay, callback dispatch,
  cancellation, and idempotent retries.
- Make the production API origin the default.

### 7. Site, dashboard, docs (`site`, `aex`)

- Apply the short page and vocabulary sweep.
- Remove the Runs UI and show all activity inside a session.
- Replace the raw HTTP quickstart with `session.output(schema)`.
- Stage on Vercel, test, and promote `aex.dev`.

### 8. Launch proof

- Execute the homepage SDK example unchanged against production.
- Verify text and typed output on every supported provider/model combination.
- Prove no schema, output-control prompt, validation issue, or repair message enters later model
  context.
- Prove one invalid output repairs once and then raises a typed error if still invalid.
- Prove an output retry after transport loss returns the same committed value.
- Create an asset, delete its session, and prove the asset still downloads.
- Interrupt a callback after dispatch and prove it is never silently replayed.
- Verify waitlist, invitation/sign-in email, API key, payment, refund, status failure email, and
  recovery email.
- Search every public surface and deployment artifact for removed vocabulary and secrets.

## Remaining owner decisions

The first option is recommended in each case.

1. **Output timing:** `output(schema, optionalInput)` performs normal work and then a private commit
   step. This costs one extra model step but keeps ordinary agent context clean.
2. **Repair:** one isolated repair attempt, then reject. No unlimited correction loop and no partial
   value masquerading as `T`.
3. **Promise API:** return a standard `Promise<T>`; use `await` or `.then/.catch`, not custom
   `.result/.error` methods.
4. **Schema support:** document Zod at alpha launch; preserve an internal Standard Schema boundary
   for other validators later.
5. **History:** persist the validated structured value as the assistant's output, but never persist
   the schema-control or repair conversation.
6. **SDK languages:** TypeScript only for alpha; Python/Pydantic immediately afterward.
7. **Assets:** immutable account assets attached to sessions; no projects, buckets, or paths yet.
8. **Computer:** session files persist; RAM, process, and machine identity do not.
9. **Contract:** replace the prelaunch `/v1` model cleanly rather than retain aliases.

If these are accepted, contract implementation can begin without another product layer.

## Research basis

- Current Claude structured outputs use `output_config.format`, constrained decoding, Zod helpers,
  and local validation against the original schema after provider-specific normalisation:
  [Claude structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs).
- Tool calling plus structured output conventionally adds a final generation step, supporting the
  clean work/commit split:
  [AI SDK tool calling with structured output](https://ai-sdk.dev/docs/troubleshooting/tool-calling-with-structured-outputs)
  and [AI SDK Output](https://ai-sdk.dev/docs/reference/ai-sdk-core/output).
- Current Pi demonstrates the provider-independent fallback: a schema-bearing tool can terminate
  the agent on its final call without an extra follow-up:
  [Pi structured-output example](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/examples/extensions/structured-output.ts)
  and [Pi extension docs](https://pi.dev/docs/latest/extensions).
- Standard Schema and Standard JSON Schema provide the future validator-neutral boundary:
  [Standard Schema](https://standardschema.dev/) and
  [MCP schema library guidance](https://ts.sdk.modelcontextprotocol.io/v2/advanced/schema-libraries).
- Private object storage and signed URLs are established access patterns for durable assets:
  [Supabase Storage access control](https://supabase.com/docs/guides/storage/security/access-control)
  and [private buckets](https://supabase.com/docs/guides/storage/buckets/fundamentals).
