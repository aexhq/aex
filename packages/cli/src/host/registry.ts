/**
 * The single source of truth for the CLI's verb surface: every subcommand, the
 * `start` flags, and the `files` sub-verbs. Two consumers read it:
 *
 *   1. Per-verb `--help` (`aex <verb> --help`) renders a static usage table
 *      from here BEFORE any auth-requiring handler runs, so discovering a
 *      verb's flags never needs an API key.
 *   2. The conformance CLI↔SDK parity manifest test asserts every SDK public
 *      capability (Aex method / session option / files accessor) maps to a verb
 *      or flag REGISTERED here — turning "mirrors the SDK" from a comment into
 *      a CI-enforced invariant.
 *
 * Keep this list in lockstep with the `dispatch()` switch in `main.ts`.
 */

export interface CliVerbSpec {
  /** The subcommand token, e.g. `start`, `files`, `delete-asset`. */
  readonly name: string;
  /** One-line description shown in per-verb help. */
  readonly summary: string;
  /** Full usage lines rendered by `aex <verb> --help`. */
  readonly usage: readonly string[];
  /**
   * Long-form flags this verb recognizes (`--model`, `--skill`, …). Drives the
   * per-verb help flag list AND the parity manifest's session-option coverage.
   */
  readonly flags?: readonly string[];
  /** Sub-verbs (e.g. `files read|download|link|find|search`). */
  readonly subverbs?: readonly string[];
}

/** Common flags every host verb accepts (rendered in per-verb help footers). */
export const COMMON_HOST_FLAGS: readonly string[] = ["--api-key", "--aex-url", "--json", "--debug"];

/**
 * The `aex start` flag surface. Also the SSoT the parity manifest maps
 * `SessionStartOptions` keys onto — a new session option the CLI should forward
 * gets a flag here, and the parity test proves the mapping is complete.
 */
export const START_FLAGS: readonly string[] = [
  "--provider",
  "--model",
  "--system",
  "--prompt",
  "--config",
  "--skill",
  "--tool",
  "--instructions",
  "--file",
  "--mcp",
  "--mcp-auth",
  "--metadata",
  "--runtime-size",
  "--runtime",
  "--session-timeout",
  "--idempotency-key",
  "--webhook",
  "--follow",
  "--timeout"
];

/** The `aex files` sub-verbs (`aex files <id>` bare = list). */
export const FILES_SUBVERBS: readonly string[] = ["read", "download", "link", "find"];

export const CLI_VERBS: readonly CliVerbSpec[] = [
  {
    name: "start",
    summary: "One-shot: open a session, send the prompt as the first turn (delegates to the SDK).",
    usage: [
      "aex start --model M --prompt P [--system S] [--provider name] --<provider>-api-key K",
      "aex start --config <session.json> --<provider>-api-key K",
      "  --skill @file        Attach a workspace skill bundle (repeatable)",
      "  --tool @file.js      Attach a custom tool module (repeatable)",
      "  --instructions @file Publish and attach session instructions (repeatable)",
      "  --file @path         Mount a file into /workspace (repeatable)",
      "  --mcp name=url       MCP server (repeatable); --mcp-auth name=Hdr:Val for headers",
      "  --metadata key=value Submission metadata (repeatable)",
      "  --runtime <kind>     Execution runtime: container | spot_container | lambda",
      "  --runtime-size <s>   Managed runtime size preset",
      "  --session-timeout <dur>  Server-side session deadline (validated client-side by the SDK)",
      "  --webhook <url>      Finalized run callback (run.finished/run.error; https)",
      "  --follow             Stream events until the run finishes"
    ],
    flags: START_FLAGS
  },
  {
    name: "status",
    summary: "Print a session record (GET /api/sessions/:id).",
    usage: ["aex status <session-id>"]
  },
  {
    name: "deliveries",
    summary: "List a session's webhook delivery attempts.",
    usage: ["aex deliveries <session-id>"]
  },
  {
    name: "wait",
    summary: "Poll a session until it stops progressing; exit code reflects lifecycle state.",
    usage: ["aex wait <session-id> [--timeout 8m] [--interval 2s]"],
    flags: ["--timeout", "--interval"]
  },
  {
    name: "events",
    summary: "List (or --follow) a session's events as NDJSON.",
    usage: ["aex events <session-id> [--follow] [--timeout 8m]"],
    flags: ["--follow", "--timeout"]
  },
  {
    name: "tail",
    summary: "Live human-readable follow over the coordinator event stream.",
    usage: ["aex tail <session-id> [--filter <type|source>] [--logs] [--from <seq>] [--timeout <dur>]"],
    flags: ["--filter", "--logs", "--from", "--timeout"]
  },
  {
    name: "inspect",
    summary: "One-shot full-timeline render + jump-to-failure summary.",
    usage: ["aex inspect <session-id> [--filter <type|source>] [--logs] [--timeout <dur>]"],
    flags: ["--filter", "--logs", "--timeout"]
  },
  {
    name: "files",
    summary: "List a session's captured files, or read/download/link/find one file.",
    usage: [
      "aex files <session-id>                         List captured files (NDJSON)",
      "aex files read <session-id> <path>             Read one file as capped text",
      "aex files download <session-id> <path> [--out] Download one file's raw bytes",
      "aex files link <session-id> <path>             Mint a temporary download URL",
      "aex files find <session-id> [--name S] [--ext E] [--type T]",
    ],
    flags: ["--out", "--name", "--ext", "--type", "--content-type", "--query", "--session-id", "--limit", "--max-bytes"],
    subverbs: FILES_SUBVERBS
  },
  {
    name: "download",
    summary: "Download a session's content as a zip (whole or one namespace).",
    usage: ["aex download <session-id> [--only files|events|metadata] [--out path]"],
    flags: ["--only", "--out"]
  },
  {
    name: "cancel",
    summary: "Cancel a running session.",
    usage: ["aex cancel <session-id>"]
  },
  {
    name: "delete",
    summary: "Delete a session.",
    usage: ["aex delete <session-id>"]
  },
  {
    name: "delete-asset",
    summary: "Delete a workspace asset blob by hash.",
    usage: ["aex delete-asset <assetId|hash>"]
  },
  {
    name: "sessions",
    summary: "List the workspace's sessions (newest first).",
    usage: ["aex sessions [--limit N] [--since ISO]"],
    flags: ["--limit", "--since"]
  },
  {
    name: "whoami",
    summary: "Resolve the API key to its workspace + scopes.",
    usage: ["aex whoami [--json]"]
  },
  {
    name: "billing",
    summary: "Show balance / spend / cap; ledger, upgrade, and portal sub-verbs.",
    usage: [
      "aex billing [--json]",
      "aex billing ledger [--limit N]",
      "aex billing upgrade pro|team",
      "aex billing portal"
    ],
    subverbs: ["ledger", "upgrade", "portal"]
  },
  {
    name: "webhooks",
    summary: "Reveal the workspace webhook signing secret.",
    usage: ["aex webhooks secret"],
    subverbs: ["secret"]
  },
  {
    name: "orgs",
    summary: "Control-plane: manage the orgs you belong to (members, invites).",
    usage: [
      "aex orgs [list]                              List your orgs",
      "aex orgs create --name <name>                Create an org (you become its admin)",
      "aex orgs members <orgId>                      List members + pending invites",
      "aex orgs invite <orgId> --email E [--role admin|member]"
    ],
    flags: ["--name", "--email", "--role"],
    subverbs: ["list", "create", "members", "invite"]
  },
  {
    name: "workspaces",
    summary: "Control-plane: manage workspaces across your orgs (create/list/delete).",
    usage: [
      "aex workspaces [list]                         List manageable workspaces",
      "aex workspaces create --org <orgId> --name N  Create + reveal its first key ONCE",
      "aex workspaces delete <workspaceId>           Delete a workspace"
    ],
    flags: ["--org", "--name"],
    subverbs: ["list", "create", "delete"]
  },
  {
    name: "keys",
    summary: "Control-plane: manage API keys — workspace keys and account PATs.",
    usage: [
      "aex keys [list]                               List key metadata (never values)",
      "aex keys create <workspaceId> [--name N]      Mint a workspace (data-plane) key",
      "aex keys create --account [--name N]          Mint an account PAT (control-plane)",
      "aex keys delete <keyId>                        Revoke a key"
    ],
    flags: ["--account", "--name"],
    subverbs: ["list", "create", "delete"]
  },
  {
    name: "login",
    summary: "Sign in via the browser device flow (or persist a workspace key with --api-key).",
    usage: [
      "aex login [--aex-url U]                       Device flow → account token (control-plane)",
      "aex login --api-key T [--aex-url U]           Persist a workspace key (data-plane)"
    ]
  },
  {
    name: "logout",
    summary: "Clear the stored token.",
    usage: ["aex logout"]
  },
  {
    name: "auth",
    summary: "Show the resolved config (token never printed).",
    usage: ["aex auth status"],
    subverbs: ["status"]
  },
  {
    name: "models",
    summary: "List models + default provider (no token needed).",
    usage: ["aex models list [--json]"]
  },
  {
    name: "providers",
    summary: "List providers + their models (no token needed).",
    usage: ["aex providers list [--json]"]
  },
  {
    name: "tools",
    summary: "List builtin tools (no token needed).",
    usage: ["aex tools list [--json]"]
  },
  {
    name: "runtime-sizes",
    summary: "List managed runtime presets (no token needed).",
    usage: ["aex runtime-sizes list [--json]"]
  }
];

const VERB_BY_NAME = new Map<string, CliVerbSpec>(CLI_VERBS.map((v) => [v.name, v]));

/** Every registered verb name (used by the parity manifest test). */
export const CLI_VERB_NAMES: readonly string[] = CLI_VERBS.map((v) => v.name);

export function findVerbSpec(name: string): CliVerbSpec | undefined {
  return VERB_BY_NAME.get(name);
}

/** Whether `argv` requests per-verb help (`--help` / `-h`). */
export function wantsVerbHelp(argv: readonly string[]): boolean {
  return argv.includes("--help") || argv.includes("-h");
}

/** Render a verb's static usage table (no API key required). */
export function renderVerbHelp(spec: CliVerbSpec): string {
  const lines: string[] = [`aex ${spec.name} — ${spec.summary}`, "", "Usage:"];
  for (const u of spec.usage) lines.push(`  ${u}`);
  lines.push("", `Common flags: ${COMMON_HOST_FLAGS.join(" ")}`);
  return lines.join("\n") + "\n";
}
