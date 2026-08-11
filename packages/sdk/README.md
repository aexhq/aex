# @aexhq/sdk

The zero-dependency TypeScript SDK for AEX sessions.

```bash
npm i @aexhq/sdk
```

```ts
import { Aex, assertId, newId } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);

const session = await aex.sessions.sessionCreate({
  body: {
    provider: "openai",
    model: process.env.AEX_MODEL!,
    providerCredentialId: assertId(
      "provider_credential",
      process.env.AEX_PROVIDER_CREDENTIAL_ID,
    ),
  },
  idempotencyKey: crypto.randomUUID(),
});

await aex.sessions.sessionMessageSend({
  sessionId: session.id,
  body: { text: "Inspect the tests and summarize the failures." },
  idempotencyKey: crypto.randomUUID(),
});

await aex.sessions.sessionSuspend({
  sessionId: session.id,
  body: {},
  operationId: newId("operation"),
});
```

The session is the customer-facing execution unit. It accepts one text message
at a time and can accept another after the current activity finishes or is
cancelled. There is no public run resource. One exact provider generation lasts
at most eight hours from launch, suspends automatically after 180 idle seconds,
and resumes for the next message or live-file request. Suspension never extends
the lifetime, and runtime loss terminates the session instead of restoring an
ambiguous partial activity.

The SDK exposes generated resource clients for bootstrap, organizations,
workspaces, API keys, billing, dedicated BYOK provider credentials, registered
workspace files, sessions and messages, live files, observations, telemetry
exports, operations, and usage. Every resource method is generated from the
published contract. `ROUTES` and `aex.execute(...)` expose that same current
route set without maintaining a second handwritten table.

Registered workspace files are durable opaque inputs. A session pins selected
file revisions at creation and materializes them into its MicroVM. Files created
inside the session are generation-local: termination or runtime loss destroys
them. The `@aexhq/sdk/node/session-files` entry provides bounded, resumable,
digest-verified upload and download helpers for those live files.

Launch has no generic secret vault, typed skill/tool/instruction/MCP registries,
interactive tool approvals, session snapshots, persistence, clone/fork, trash,
or restore surface. Provider keys use the dedicated write-only BYOK authority.
The built-in model tools are exactly `read_file`, `edit_file`, `write_file`, and
Bash; guidance and configuration can be supplied as opaque workspace files.

Telemetry and telemetry exports are durable even though live session files and
execution state are not.

The native `aex` command is distributed as a signed platform archive.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
