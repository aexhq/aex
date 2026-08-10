# @aexhq/sdk

The TypeScript SDK for explicit aex v1 sessions.

```bash
npm i @aexhq/sdk
```

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const runs = await aex.sessions.sessionRunsList({ sessionId: "ses_…" });
console.log(runs);
```

The SDK exposes explicit bootstrap resources (`account`, `organizations`,
`workspaces`, `apiKeys`, and `billing`) and region-pinned workspace resources
(`sessions`, operations, files, registries, secrets, approvals, telemetry, and
usage). Long-running mutations return durable operation handles.

Every resource method is generated from the published contract, one per
operation the platform serves. An operation the contract declares and nothing
serves yet has **no method**: a method that could only ever fail would put the
platform's answer in the client, where it goes stale the day the route lands.
`ROUTES` still carries all of them with a `deferred` flag, and
`aex.execute(routeId, …)` stays total over every route id, so a deferred
operation remains callable by anyone who wants to see the `501` for themselves.

The SDK exposes only the strict v1 resource model. Registered inputs are
overwrite-by-name resources whose PUT result reports `created`, `replaced`, or
`unchanged`. Returned byte-bearing values contain checksum/size descriptors,
never inline bytes or upload IDs. Persisted/live file reads, current registered
file downloads, and telemetry exports remain explicit.

The native `aex` command is distributed as a signed platform archive.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
