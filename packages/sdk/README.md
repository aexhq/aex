# @aexhq/sdk

The TypeScript SDK for explicit aex v1 sessions.

```bash
npm i @aexhq/sdk
```

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  provider: "anthropic",
  model: "claude-haiku-4-5"
});

const { run } = await session.messages.send("Summarize this repository.");
const result = await run.result();
console.log(result.status);
```

The SDK exposes explicit bootstrap resources (`account`, `organizations`,
`workspaces`, `apiKeys`, and `billing`) and region-pinned workspace resources
(`sessions`, operations, files, registries, secrets, approvals, telemetry, and
usage). Long-running mutations return durable operation handles.

The SDK exposes only the strict v1 resource model. Registered inputs are
overwrite-by-name resources whose PUT result reports `created`, `replaced`, or
`unchanged`. Returned byte-bearing values contain checksum/size descriptors,
never inline bytes or upload IDs. Persisted/live file reads, current registered
file downloads, and telemetry exports remain explicit.

The native `aex` command is distributed as a signed platform archive.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
