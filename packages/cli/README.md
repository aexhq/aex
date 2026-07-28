# @aexhq/cli

The aex command-line interface. Start a session, follow its event stream, and
move files in and out of it from a terminal or a shell script.

```bash
npx @aexhq/cli start \
  --api-key "$AEX_API_KEY" \
  --model anthropic/claude-haiku-4-5 \
  --prompt "Write a short report." \
  --follow
```

## Install exactly one of these

This package and `@aexhq/sdk` both install a command called `aex`, and they
install **the same binary**: the SDK's `dist/cli.mjs` is a copy of the bundle
built here. Installing both in one tree leaves the winner to package-manager
ordering.

| You want | Install |
| --- | --- |
| The CLI only | `@aexhq/cli` |
| The TypeScript SDK, with the CLI included | `@aexhq/sdk` |

There is no third option and no reason to install both.

## What it is for

The CLI is a thin, fully typed front end over the same public contracts the SDK
uses (`@aexhq/contracts`). It does not hold any capability the SDK lacks — it
exists so an agent, a CI job, or a person can drive a session without writing
TypeScript.

The generated command reference lives at
[aex.dev/docs/reference/cli](https://aex.dev/docs/reference/cli/); it is
produced from `aex --help`, so it cannot drift from the binary.

## Programmatic entry points

Two subpaths are exported for embedding, and nothing else is public:

- `@aexhq/cli` — `executeCli`, the verb table, and the flag specifications.
- `@aexhq/cli/runtime` — the file-sync command, for callers running inside a
  session runtime.

Deep imports are blocked by the export map on purpose;
`test/package-surface.test.ts` asserts that a published tarball keeps them
blocked.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
