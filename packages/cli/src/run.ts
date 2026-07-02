/**
 * aex CLI — main runner. Pure (no `process.*` reads), gets all IO
 * via the injected {@link CliIO} surface. The thin entrypoint in
 * `cli.ts` wires real stdin/stdout/fetch/readFile and calls `runCli()`.
 *
 * Subcommands:
 *   Host:
 *     - `aex run --config <run.json> [flags]`
 *     - `aex status <session-id>`
 *     - `aex deliveries <session-id>`
 *     - `aex wait <session-id> [--timeout <dur>] [--interval <dur>]`
 *     - `aex events <session-id> [--follow] [--timeout <dur>]`
 *     - `aex tail <session-id> [--json] [--filter ...] [--logs] [--settle] [--timeout <dur>]`
 *     - `aex inspect <session-id> [--json] [--filter ...] [--logs] [--timeout <dur>]`
 *     - `aex outputs <session-id>`
 *     - `aex download <session-id> [--only outputs|events|metadata] [--out path]`
 *     - `aex cancel <session-id>`
 *     - `aex delete <session-id>`
 *     - `aex whoami`
 *     - `aex redeem <code>`
 *     - `aex login` / `aex logout` / `aex auth status`
 *     - `aex models|providers|tools|runtime-sizes list` (no token needed)
 *
 *   Operator (AWS creds, not `--api-token`):
 *     - `aex debug <run-id> [--plane dev|prd] [--region eu-west-2] [--cloudwatch]`
 *
 * Every host subcommand (except the operator `debug` verb) requires
 * `--api-token`. `--aex-url` is
 * optional and defaults to `https://api.aex.dev`. There is no
 * `--workspace` flag — the workspace is derived server-side from the
 * API token.
 */
import { RUN_PROVIDERS } from "@aexhq/contracts";
import type { CliIO } from "./internal.js";
import { runOutputsSyncCmd } from "./outputs-sync.js";
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
  runStatusCmd,
  runDeliveriesCmd,
  runWaitCmd,
  runWhoamiCmd,
  runRedeemCmd,
  runDebugCmd,
  runLoginCmd,
  runLogoutCmd,
  runAuthStatusCmd,
  runModelsCmd,
  runProvidersCmd,
  runToolsCmd,
  runRuntimeSizesCmd,
  runTailCmd,
  runInspectCmd
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
    const body = { error: "internal_error", message: (err as Error).message ?? "unknown error" };
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
    case "run":
      return runRunCmd(io, rest);
    case "status":
      return runStatusCmd(io, rest);
    case "deliveries":
      return runDeliveriesCmd(io, rest);
    case "wait":
      return runWaitCmd(io, rest);
    case "events":
      return runEventsCmd(io, rest);
    case "tail":
      // Live human-readable follow over the coordinator WS envelope stream.
      return runTailCmd(io, rest);
    case "inspect":
      // One-shot full-timeline render + summary/jump-to-failure (WS stream).
      return runInspectCmd(io, rest);
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
    case "redeem":
      // Redeem a single-use coupon code to fund the workspace prepaid balance.
      // CLI-only (direct fetch); intentionally not a public SDK method.
      return runRedeemCmd(io, rest);
    case "login":
      return runLoginCmd(io, rest);
    case "logout":
      return runLogoutCmd(io, rest);
    case "auth":
      // `aex auth status` (default subcommand `status`). Token never printed.
      return runAuthStatusCmd(io, rest[0] === "status" ? rest.slice(1) : rest);
    case "models":
      // Discoverability reads of the contracts SSoT — no token, no network.
      // Each accepts an optional `list` subcommand and `--json`.
      return runModelsCmd(io, rest);
    case "providers":
      return runProvidersCmd(io, rest);
    case "tools":
      return runToolsCmd(io, rest);
    case "runtime-sizes":
      return runRuntimeSizesCmd(io, rest);
    case "debug":
      // Operator/admin command: reads the AWS plane directly (S3 + DDB + SFN +
      // CloudWatch) via the standard AWS SDK credential chain. NOT an
      // --api-token verb — distinct from the public host commands above.
      return runDebugCmd(io, rest);
    default:
      io.stderr(`unknown subcommand: ${sub}\n`);
      io.stderr("run `aex --help` for usage\n");
      return USAGE_ERR;
  }
}

async function printGlobalHelp(io: CliIO): Promise<CliExitCode> {
  // Host-side help: the unified surface mirroring the SDK 1:1.
  io.stdout("aex — unified CLI for the aex platform (mirrors the SDK 1:1)\n\n");
  io.stdout("Usage:\n");
  io.stdout("  aex run --config <run.json> --<provider>-api-key K --api-token T [flags]\n");
  io.stdout("  aex run --model M --prompt P [--system S] [--mcp name=url ...] --<provider>-api-key K --api-token T [flags]\n");
  io.stdout("  aex status <session-id> --api-token T\n");
  io.stdout("  aex deliveries <session-id> --api-token T\n");
  io.stdout("  aex wait <session-id> [--timeout 8m] [--interval 2s] --api-token T\n");
  io.stdout("  aex events <session-id> [--follow] [--timeout 8m] --api-token T\n");
  io.stdout("  aex tail <session-id> [--json] [--filter <type|source>] [--logs] [--settle] [--timeout 8m] --api-token T\n");
  io.stdout("  aex inspect <session-id> [--json] [--filter <type|source>] [--logs] [--timeout 8m] --api-token T\n");
  io.stdout("  aex outputs <session-id> --api-token T\n");
  io.stdout("  aex download <session-id> [--only outputs|events|metadata] [--out path] --api-token T\n");
  io.stdout("  aex cancel <session-id> --api-token T\n");
  io.stdout("  aex delete <session-id> --api-token T\n");
  io.stdout("  aex delete-asset <assetId|hash> --api-token T\n");
  io.stdout("  aex whoami --api-token T\n");
  io.stdout("  aex redeem <code> --api-token T             Redeem a coupon code into the workspace prepaid balance\n");
  io.stdout("  aex login --api-token T [--aex-url U]      Persist token + url (then other verbs need no --api-token)\n");
  io.stdout("  aex logout                                 Clear the stored token\n");
  io.stdout("  aex auth status                            Show the resolved config (token never printed)\n");
  io.stdout("  aex models list [--json]                   List models + default provider (no token needed)\n");
  io.stdout("  aex providers list [--json]                List providers + their models (no token needed)\n");
  io.stdout("  aex tools list [--json]                    List builtin tools (all default; no token needed)\n");
  io.stdout("  aex runtime-sizes list [--json]            List managed runtime presets (no token needed)\n");
  io.stdout("  aex debug <run-id> [--plane dev|prd] [--region eu-west-2] [--cloudwatch] [--with-outputs]   (operator; AWS creds)\n");
  io.stdout("  aex --help\n\n");
  io.stdout("Common flags on every host subcommand:\n");
  io.stdout("  --api-token <token>         REQUIRED — aex SDK API token (workspace is derived from it)\n");
  io.stdout("  --aex-url <url>         Optional; defaults to https://api.aex.dev (local/staging/hosted plane)\n");
  io.stdout("  --debug                     Optional; print a redacted per-request trace to stderr (uploads nothing)\n\n");
  io.stdout("aex run flags:\n");
  io.stdout(`  --provider <name>           Optional; one of: ${RUN_PROVIDERS.join(", ")} (default anthropic)\n`);
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
  io.stdout("  --runtime-size <size>       managed runtime preset\n");
  io.stdout("  --run-timeout <dur>         Server-side run deadline (e.g. 1h); distinct from --timeout\n");
  io.stdout("  --idempotency-key <key>     Optional; defaults to a fresh UUID\n");
  io.stdout("  --webhook <url>             Optional per-run callback URL (https); receives the terminal run.finished event\n");
  io.stdout("  --follow                    Poll events to stdout until the run terminates\n");
  io.stdout("  --timeout <dur>             With --follow: give up after this long (e.g. 8m, 30s, 500ms); exit code 3\n");
  return SUCCESS;
}
