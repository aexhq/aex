/**
 * aex CLI — main runner. Pure (no `process.*` reads), gets all IO
 * via the injected {@link CliIO} surface. The thin entrypoint in
 * `cli.ts` wires real stdin/stdout/fetch/readFile and calls `executeCli()`.
 *
 * Subcommands:
 *   Host:
 *     - `aex start --config <session.json> [flags]`
 *     - `aex status <session-id>`
 *     - `aex deliveries <session-id>`
 *     - `aex wait <session-id> [--timeout <dur>] [--interval <dur>]`
 *     - `aex events <session-id> [--follow] [--timeout <dur>]`
 *     - `aex tail <session-id> [--json] [--filter ...] [--logs] [--timeout <dur>]`
 *     - `aex inspect <session-id> [--json] [--filter ...] [--logs] [--timeout <dur>]`
 *     - `aex files <session-id>`
 *     - `aex download <session-id> [--only files|events|metadata] [--out path]`
 *     - `aex cancel <session-id>`
 *     - `aex delete <session-id>`
 *     - `aex sessions [--limit <n>] [--since <iso>]`
 *     - `aex whoami`
 *     - `aex billing [--json]` / `aex billing ledger [--limit <n>]`
 *     - `aex billing upgrade pro|team` / `aex billing portal`
 *     - `aex webhooks secret`
 *     - `aex login` / `aex logout` / `aex auth status`
 *     - `aex models|providers|tools|runtime-sizes list` (no token needed)
 *
 * Authenticated host subcommands resolve an explicit `--api-key` first, then a
 * key saved by `aex login`. Local discovery and help commands need no token.
 * `--aex-url` is optional and defaults to the canonical URL for the key's dev
 * or prd plane. There is no `--workspace` flag — the workspace is derived
 * server-side from the API key.
 */
import { PROVIDERS } from "@aexhq/contracts";
import type { CliIO } from "./internal.js";
import { executeFilesSyncCmd } from "./files-sync.js";
import { findVerbSpec, renderVerbHelp, wantsVerbHelp } from "./host/registry.js";
import {
  RUNTIME_ERR,
  SUCCESS,
  USAGE_ERR,
  type CliExitCode,
  executeCancelCmd,
  executeDeleteCmd,
  executeDeleteAssetCmd,
  executeDownloadCmd,
  executeEventsCmd,
  executeSessionFilesCmd,
  executeStartCmd,
  executeStatusCmd,
  executeDeliveriesCmd,
  executeWaitCmd,
  executeWhoamiCmd,
  executeBillingCmd,
  sessionWebhooksCmd,
  executeSessionsCmd,
  executeLoginCmd,
  executeLogoutCmd,
  executeAuthStatusCmd,
  modelNamesCmd,
  providerNamesCmd,
  executeToolsCmd,
  executeRuntimeSizesCmd,
  executeTailCmd,
  executeInspectCmd
} from "./host/index.js";

export type { CliExitCode } from "./host/common.js";

/**
 * Top-level CLI entrypoint. Parses argv, dispatches to a subcommand
 * handler, returns the desired exit code. Calls `io.exit(code)` itself
 * so the entrypoint can simply `await executeCli(io)`.
 */
export async function executeCli(io: CliIO): Promise<void> {
  const args = io.argv.slice(2); // skip runtime + script
  try {
    const exit = await dispatch(io, args);
    io.exit(exit.code);
  } catch (err) {
    // Defense in depth — executeCli should never throw. If it does, emit a
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
  // Per-verb `--help`/`-h`: rendered from the static verb registry BEFORE the
  // auth-requiring handler executes, so discovering a verb's flags never needs an
  // API key (T6f). The `files sync` in-container internal verb is exempt.
  const spec = sub === undefined ? undefined : findVerbSpec(sub);
  if (spec && wantsVerbHelp(rest) && !(sub === "files" && rest[0] === "sync")) {
    io.stdout(renderVerbHelp(spec));
    return SUCCESS;
  }
  switch (sub) {
    case "start":
      return executeStartCmd(io, rest);
    case "status":
      return executeStatusCmd(io, rest);
    case "deliveries":
      return executeDeliveriesCmd(io, rest);
    case "wait":
      return executeWaitCmd(io, rest);
    case "events":
      return executeEventsCmd(io, rest);
    case "tail":
      // Live human-readable follow over the coordinator WS envelope stream.
      return executeTailCmd(io, rest);
    case "inspect":
      // One-shot full-timeline render + summary/jump-to-failure (WS stream).
      return executeInspectCmd(io, rest);
    case "files":
      // `files sync <dirs>` is the in-container internal
      // capture walker. The bare `files <session-id>` form is the
      // host-side list verb. We
      // distinguish on the first sub-arg rather than on
      // manifest-presence so a misconfigured host invocation
      // (e.g. `aex files sync ...` on a developer machine)
      // produces a clear "in-container only" error instead of
      // silently routing to the wrong handler.
      if (rest[0] === "sync") {
        return executeFilesSyncCmd(io, rest.slice(1));
      }
      return executeSessionFilesCmd(io, rest);
    case "download":
      return executeDownloadCmd(io, rest);
    case "cancel":
      return executeCancelCmd(io, rest);
    case "delete":
      return executeDeleteCmd(io, rest);
    case "delete-asset":
      return executeDeleteAssetCmd(io, rest);
    case "sessions":
      // Workspace session list (newest first) via GET /api/sessions.
      return executeSessionsCmd(io, rest);
    case "whoami":
      return executeWhoamiCmd(io, rest);
    case "billing":
      // Balance / month spend / spend cap (and `billing ledger` rows).
      return executeBillingCmd(io, rest);
    case "webhooks":
      // `aex webhooks secret` — reveal the workspace webhook signing secret.
      return sessionWebhooksCmd(io, rest);
    case "login":
      return executeLoginCmd(io, rest);
    case "logout":
      return executeLogoutCmd(io, rest);
    case "auth":
      // `aex auth status` (default subcommand `status`). Token never printed.
      return executeAuthStatusCmd(io, rest[0] === "status" ? rest.slice(1) : rest);
    case "models":
      // Discoverability reads of the contracts SSoT — no token, no network.
      // Each accepts an optional `list` subcommand and `--json`.
      return modelNamesCmd(io, rest);
    case "providers":
      return providerNamesCmd(io, rest);
    case "tools":
      return executeToolsCmd(io, rest);
    case "runtime-sizes":
      return executeRuntimeSizesCmd(io, rest);
    default:
      io.stderr(`unknown subcommand: ${sub}\n`);
      io.stderr("use `aex --help` for usage\n");
      return USAGE_ERR;
  }
}

async function printGlobalHelp(io: CliIO): Promise<CliExitCode> {
  // Host-side help: the unified surface over the aex SDK. Capability parity
  // (every SDK method/session-option/files accessor has a verb/flag) is enforced
  // by the conformance `cli-sdk-parity` manifest test.
  io.stdout("aex — unified CLI for the aex platform (a thin pass-through over the SDK)\n\n");
  io.stdout("Usage:\n");
  io.stdout("  aex start --config <session.json> --<provider>-api-key K --api-key T [flags]\n");
  io.stdout("  aex start --model M --prompt P [--system S] [--mcp name=url ...] --<provider>-api-key K --api-key T [flags]\n");
  io.stdout("  aex status <session-id> --api-key T\n");
  io.stdout("  aex deliveries <session-id> --api-key T\n");
  io.stdout("  aex wait <session-id> [--timeout 8m] [--interval 2s] --api-key T\n");
  io.stdout("  aex events <session-id> [--follow] [--timeout 8m] --api-key T\n");
  io.stdout("  aex tail <session-id> [--json] [--filter <type|source>] [--logs] [--timeout 8m] --api-key T\n");
  io.stdout("  aex inspect <session-id> [--json] [--filter <type|source>] [--logs] [--timeout 8m] --api-key T\n");
  io.stdout("  aex files <session-id> --api-key T\n");
  io.stdout("  aex download <session-id> [--only files|events|metadata] [--out path] --api-key T\n");
  io.stdout("  aex cancel <session-id> --api-key T\n");
  io.stdout("  aex delete <session-id> --api-key T\n");
  io.stdout("  aex delete-asset <assetId|hash> --api-key T\n");
  io.stdout("  aex sessions [--limit N] [--since ISO] --api-key T      List the workspace's sessions (newest first, JSON)\n");
  io.stdout("  aex whoami --api-key T\n");
  io.stdout("  aex billing [--json] --api-key T          Show prepaid balance, month spend, and spend cap\n");
  io.stdout("  aex billing ledger [--limit N] --api-key T   Recent credit-ledger entries (newest first, JSON)\n");
  io.stdout("  aex billing upgrade pro|team --api-key T   Create a hosted checkout session and print its URL\n");
  io.stdout("  aex billing portal --api-key T             Create a hosted billing portal session and print its URL\n");
  io.stdout("  aex webhooks secret --api-key T           Reveal (create on first use) the webhook signing secret\n");
  io.stdout("  aex login --api-key T [--aex-url U]      Persist token + url (then other verbs need no --api-key)\n");
  io.stdout("  aex logout                                 Clear the stored token\n");
  io.stdout("  aex auth status                            Show the resolved config (token never printed)\n");
  io.stdout("  aex models list [--json]                   List models + default provider (no token needed)\n");
  io.stdout("  aex providers list [--json]                List providers + their models (no token needed)\n");
  io.stdout("  aex tools list [--json]                    List builtin tools (all default; no token needed)\n");
  io.stdout("  aex runtime-sizes list [--json]              List managed runtime presets (no token needed)\n");
  io.stdout("  aex --help\n\n");
  io.stdout("Common flags on every host subcommand:\n");
  io.stdout("  --api-key <token>         aex SDK API key; optional after `aex login` (workspace is derived from it)\n");
  io.stdout("  --aex-url <url>         Optional; defaults by key plane (prd https://api.aex.dev; dev https://dev-api.aex.dev)\n");
  io.stdout("  --debug                     Optional; print a redacted per-request trace to stderr (uploads nothing)\n\n");
  io.stdout("aex start flags:\n");
  io.stdout(`  --provider <name>           Optional; one of: ${PROVIDERS.join(", ")} (default anthropic)\n`);
  for (const provider of PROVIDERS) {
    io.stdout(`  --${provider}-api-key <key>${" ".repeat(Math.max(1, 13 - provider.length))}REQUIRED when --provider ${provider}; never stored\n`);
  }
  io.stdout("  --config <path>             Session request JSON (mutually exclusive with the flat --model/--prompt flags)\n");
  io.stdout("  --model <model-id>          Provider model id (required in flat mode)\n");
  io.stdout("  --system @file | <text>     System message; @-prefix reads from file\n");
  io.stdout("  --prompt @file | <text>     User message; @-prefix reads from file (repeatable)\n");
  io.stdout("  --mcp name=url              MCP server entry (repeatable)\n");
  io.stdout("  --mcp-auth name=Hdr:Val     Auth header on the matching --mcp; routed into vaulted secrets (repeatable)\n");
  io.stdout("  --metadata key=value        Submission metadata entry (repeatable)\n");
  io.stdout("  --runtime <kind>            execution runtime: container | spot_container | lambda (default container)\n");
  io.stdout("  --runtime-size <size>       managed runtime size preset\n");
  io.stdout("  --session-timeout <dur>     Server-side session deadline (e.g. 1h, max 8h); distinct from --timeout\n");
  io.stdout("  --idempotency-key <key>     Optional; defaults to a fresh UUID\n");
  io.stdout("  --webhook <url>             Optional session callback URL (https); receives each run.finished/run.error event\n");
  io.stdout("  --follow                    Poll events to stdout until the current run finishes\n");
  io.stdout("  --timeout <dur>             With --follow: give up after this long (e.g. 8m, 30s, 500ms); exit code 3\n");
  return SUCCESS;
}
