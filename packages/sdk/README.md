# @aexhq/sdk

The TypeScript SDK for explicit aex v1 sessions.

```bash
npm i @aexhq/sdk
```

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});

const { run } = await session.messages.send("Summarize this repository.");
const result = await run.result();
console.log(result.status);
```

The SDK exposes explicit bootstrap resources (`account`, `organizations`,
`workspaces`, `apiKeys`, and `billing`) and region-pinned workspace resources
(`sessions`, operations, files, registries, secrets, approvals, telemetry, and
usage). Long-running mutations return durable operation handles.

There is no one-shot `start`, runtime selector, checkpoint/suspend/resume,
child-session, webhook, archive, polling-event, combined workspace-key
creation, or legacy subscription surface.

The `aex` command is published separately by `@aexhq/cli`.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
