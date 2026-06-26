/**
 * aex CLI — main runner. Pure (no `process.*` reads), gets all IO
 * via the injected {@link CliIO} surface. The thin entrypoint in
 * `cli.ts` wires real stdin/stdout/fetch/readFile and calls `runCli()`.
 *
 * Subcommands:
 *   In-container (manifest at AEX_INDEX_PATH —
 *   `/mnt/session/uploads/aex/index.json` — present):
 *     - `aex proxy <endpoint-name> [flags]`
 *
 *   Host (manifest absent):
 *     - `aex run --config <run.json> [flags]`
 *     - `aex status <run-id>`
 *     - `aex deliveries <run-id>`
 *     - `aex wait <run-id> [--timeout <dur>] [--interval <dur>]`
 *     - `aex events <run-id> [--follow] [--timeout <dur>]`
 *     - `aex outputs <run-id>`
 *     - `aex download <run-id> [--only outputs|events|metadata] [--out path]`
 *     - `aex cancel <run-id>`
 *     - `aex delete <run-id>`
 *     - `aex whoami`
 *
 * Every host subcommand requires `--api-token`. `--aex-url` is
 * optional and defaults to `https://api.aex.dev`. There is no
 * `--workspace` flag — the workspace is derived server-side from the
 * API token.
 */
import { RUN_PROVIDERS, REGIONS, type ProxyErrorBody } from "@aexhq/contracts";
import type { CliIO } from "./internal.js";
import { runOutputsSyncCmd } from "./outputs-sync.js";
import { formatProxyEndpointSummary, printProxyHelp, runProxy, tryReadManifest } from "./proxy.js";
import {
  RUNTIME_ERR,
  SUCCESS,
  USAGE_ERR,
  type CliExitCode,
  runCancelCmd,
  runDeleteCmd,
  runDeleteAssetCmd,
  runDownloadCmd,
  runEventsCmd,
  runOutputsCmd,
  runRunCmd,
  runSkillsCmd,
  runStatusCmd,
  runDeliveriesCmd,
  runWaitCmd,
  runWhoamiCmd
} from "./host/index.js";

export type { CliExitCode } from "./host/common.js";

/**
 * Top-level CLI entrypoint. Parses argv, dispatches to a subcommand
 * handler, returns the desired exit code. Calls `io.exit(code)` itself
 * so the entrypoint can simply `await runCli(io)`.
 */
export async function runCli(io: CliIO): Promise<void> {
  const args = io.argv.slice(2); // skip runtime + script
  try {
    const exit = await dispatch(io, args);
    io.exit(exit.code);
  } catch (err) {
    // Defense in depth — runCli should never throw. If it does, emit a
    // stable error envelope rather than a stack trace.
    const body: ProxyErrorBody = { error: "internal_error", message: (err as Error).message ?? "unknown error" };
    io.stderr(JSON.stringify(body) + "\n");
    io.exit(RUNTIME_ERR.code);
  }
}

async function dispatch(io: CliIO, args: readonly string[]): Promise<CliExitCode> {
  if (args.length === 0 || args[0] === "--help" || args[0] === "-h") {
    return printGlobalHelp(io);
  }
  const sub = args[0];
  const rest = args.slice(1);
  switch (sub) {
    case "proxy":
      return runProxy(io, rest);
    case "run":
      return runRunCmd(io, rest);
    case "skills":
      return runSkillsCmd(io, rest);
    case "status":
      return runStatusCmd(io, rest);
    case "deliveries":
      return runDeliveriesCmd(io, rest);
    case "wait":
      return runWaitCmd(io, rest);
    case "events":
      return runEventsCmd(io, rest);
    case "outputs":
      // `outputs sync <dirs>` is the legacy in-container internal
      // capture walker. The bare `outputs <run-id>` form is the
      // host-side list verb. We
      // distinguish on the first sub-arg rather than on
      // manifest-presence so a misconfigured host invocation
      // (e.g. `aex outputs sync ...` on a developer machine)
      // produces a clear "in-container only" error instead of
      // silently routing to the wrong handler.
      if (rest[0] === "sync") {
        return runOutputsSyncCmd(io, rest.slice(1));
      }
      return runOutputsCmd(io, rest);
    case "download":
      return runDownloadCmd(io, rest);
    case "cancel":
      return runCancelCmd(io, rest);
    case "delete":
      return runDeleteCmd(io, rest);
    case "delete-asset":
      return runDeleteAssetCmd(io, rest);
    case "whoami":
      return runWhoamiCmd(io, rest);
    default:
      io.stderr(`unknown subcommand: ${sub}\n`);
      io.stderr("run `aex --help` for usage\n");
      return USAGE_ERR;
  }
}

async function printGlobalHelp(io: CliIO): Promise<CliExitCode> {
  const manifest = await tryReadManifest(io);
  if (manifest) {
    // In-container help — only `proxy` is reachable from inside a run.
    io.stdout("aex — in-container CLI for managed run sessions\n\n");
    io.stdout("Usage:\n");
    io.stdout("  aex proxy <endpoint-name> [flags]\n");
    io.stdout("  aex proxy --help\n\n");
    if (manifest.endpoints.length === 0) {
      io.stdout("This run declared no proxy endpoints.\n");
    } else {
      io.stdout("Declared proxy endpoints for this run:\n");
      for (const ep of manifest.endpoints) {
        io.stdout(`  • ${formatProxyEndpointSummary(ep)}\n`);
      }
    }
    io.stdout(`\nProtocol version: ${manifest.protocolVersion}\n`);
    return SUCCESS;
  }

  // Host-side help: the unified surface mirroring the SDK 1:1.
  io.stdout("aex — unified CLI for the aex platform (mirrors the SDK 1:1)\n\n");
  io.stdout("Usage:\n");
  io.stdout("  aex run --config <run.json> --<provider>-api-key K --api-token T [flags]\n");
  io.stdout("  aex run --model M --prompt P [--system S] [--mcp name=url ...] --<provider>-api-key K --api-token T [flags]\n");
  io.stdout("  aex skills upload --name N --from-path <dir> --api-token T\n");
  io.stdout("  aex skills upload --name N --file <path> [--file <path> ...] --api-token T\n");
  io.stdout("  aex skills list --api-token T\n");
  io.stdout("  aex skills get <skill-id> --api-token T\n");
  io.stdout("  aex skills delete <skill-id> --api-token T\n");
  io.stdout("  aex status <run-id> --api-token T\n");
  io.stdout("  aex deliveries <run-id> --api-token T\n");
  io.stdout("  aex wait <run-id> [--timeout 8m] [--interval 2s] --api-token T\n");
  io.stdout("  aex events <run-id> [--follow] [--timeout 8m] --api-token T\n");
  io.stdout("  aex outputs <run-id> --api-token T\n");
  io.stdout("  aex download <run-id> [--only outputs|events|metadata] [--out path] --api-token T\n");
  io.stdout("  aex cancel <run-id> --api-token T\n");
  io.stdout("  aex delete <run-id> --api-token T\n");
  io.stdout("  aex delete-asset <assetId|hash> --api-token T\n");
  io.stdout("  aex whoami --api-token T\n");
  io.stdout("  aex --help\n\n");
  io.stdout("Common flags on every host subcommand:\n");
  io.stdout("  --api-token <token>         REQUIRED — aex SDK API token (workspace is derived from it)\n");
  io.stdout("  --aex-url <url>         Optional; defaults to https://api.aex.dev (local/staging/hosted plane)\n");
  io.stdout("  --debug                     Optional; print a redacted per-request trace to stderr (uploads nothing)\n\n");
  io.stdout("aex run flags:\n");
  io.stdout(`  --provider <name>           Optional; one of: ${RUN_PROVIDERS.join(", ")} (default anthropic)\n`);
  io.stdout("  --runtime managed           Optional runtime selector; omitted also uses managed\n");
  for (const provider of RUN_PROVIDERS) {
    io.stdout(`  --${provider}-api-key <key>${" ".repeat(Math.max(1, 13 - provider.length))}REQUIRED when --provider ${provider}; never stored\n`);
  }
  io.stdout("  --config <path>             Run-request JSON (mutually exclusive with the flat --model/--prompt flags)\n");
  io.stdout("  --model <model-id>          Provider model id (required in flat mode)\n");
  io.stdout("  --system @file | <text>     System message; @-prefix reads from file\n");
  io.stdout("  --prompt @file | <text>     User message; @-prefix reads from file (repeatable)\n");
  io.stdout("  --mcp name=url              MCP server entry (repeatable)\n");
  io.stdout("  --mcp-auth name=Hdr:Val     Auth header on the matching --mcp; routed into vaulted secrets (repeatable)\n");
  io.stdout("  --metadata key=value        Submission metadata entry (repeatable)\n");
  io.stdout("  --proxy-endpoint '<json>'   PlatformProxyEndpoint JSON (repeatable)\n");
  io.stdout("  --proxy-auth name=<spec>    bearer:tok | basic:u:p | header:v | query:v (repeatable)\n");
  io.stdout(`  --region <region>           Product placement region; one of: ${REGIONS.join(", ")}\n`);
  io.stdout("  --runtime-size <size>       managed runtime preset\n");
  io.stdout("  --run-timeout <dur>         Server-side run deadline (e.g. 1h); distinct from --timeout\n");
  io.stdout("  --idempotency-key <key>     Optional; defaults to a fresh UUID\n");
  io.stdout("  --webhook <url>             Optional per-run callback URL (https); receives the terminal run.finished event\n");
  io.stdout("  --follow                    Poll events to stdout until the run terminates\n");
  io.stdout("  --timeout <dur>             With --follow: give up after this long (e.g. 8m, 30s, 500ms); exit code 3\n");
  return SUCCESS;
}

// Re-export the proxy printer so existing imports keep working.
export { printProxyHelp };
