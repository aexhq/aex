/**
 * Shared helpers for every host-side antpath subcommand. Common flag
 * parsing, HttpClient construction, manifest detection so we can refuse
 * to run host commands inside a managed run container, and exit codes
 * shared with the in-container proxy command.
 */
import { ANTPATH_DEFAULT_BASE_URL, HttpClient, type FetchLike } from "@antpath/contracts";
import { ANTPATH_INDEX_PATH, type CliIO } from "../internal.js";

export interface CliExitCode {
  readonly code: number;
}

export const SUCCESS: CliExitCode = { code: 0 };
export const USAGE_ERR: CliExitCode = { code: 2 };
export const RUNTIME_ERR: CliExitCode = { code: 1 };
/**
 * Distinct exit code for "the wait/follow deadline elapsed before the
 * run reached a terminal status". Separated from RUNTIME_ERR (1) so a
 * script can tell a timeout apart from a run that finished non-succeeded
 * (which is also RUNTIME_ERR). Mirrors the SDK's `waitForRun` throwing a
 * dedicated timeout error.
 */
export const TIMEOUT_ERR: CliExitCode = { code: 3 };

export interface CommonHostFlags {
  readonly apiToken: string;
  readonly antpathUrl: string;
  /** `--debug`: print a redacted per-request trace to stderr. Uploads nothing. */
  readonly debug: boolean;
}

export type ParseCommonResult =
  | { readonly ok: true; readonly flags: CommonHostFlags; readonly rest: readonly string[] }
  | { readonly ok: false; readonly reason: string };

/**
 * Parse and remove the common flags every host-side subcommand needs,
 * leaving the rest for the caller.
 *
 * The CLI is `flags_only` — no `ANTPATH_*` env reads. `--api-token` is
 * required; `--antpath-url` defaults to `ANTPATH_DEFAULT_BASE_URL`
 * (`https://api.antpath.ai`) so SaaS users never need to supply it.
 *
 * There is no `--workspace` flag: workspace identity is derived
 * server-side from the API token.
 */
export function parseCommonHostFlags(argv: readonly string[]): ParseCommonResult {
  let apiToken: string | null = null;
  let antpathUrl: string | null = null;
  let debug = false;
  const rest: string[] = [];

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg === "--debug") {
      debug = true;
      continue;
    }
    if (arg === "--api-token") {
      const v = argv[++i];
      if (v === undefined) return { ok: false, reason: "--api-token requires a value" };
      apiToken = v;
      continue;
    }
    if (arg === "--antpath-url") {
      const v = argv[++i];
      if (v === undefined) return { ok: false, reason: "--antpath-url requires a value" };
      antpathUrl = v;
      continue;
    }
    if (arg === "--workspace" || arg === "--workspace-id") {
      // Removed in favor of server-side derivation from the API token.
      // Bail out early with an actionable message instead of letting
      // the flag fall through and produce a confusing positional-arg
      // count error from the calling subcommand.
      return {
        ok: false,
        reason: `unknown flag ${arg}: workspace is derived from --api-token on the server; drop this flag`
      };
    }
    rest.push(arg);
  }

  if (!apiToken) return { ok: false, reason: "--api-token is required" };
  return {
    ok: true,
    flags: { apiToken, antpathUrl: antpathUrl ?? ANTPATH_DEFAULT_BASE_URL, debug },
    rest
  };
}

export function makeHttpClient(io: CliIO, flags: CommonHostFlags): HttpClient {
  return new HttpClient({
    baseUrl: flags.antpathUrl,
    apiToken: flags.apiToken,
    fetch: io.fetchImpl as FetchLike,
    // `--debug`: route the transport's redacted per-request traces to stderr.
    ...(flags.debug ? { debug: (line: string) => io.stderr(`${line}\n`) } : {})
  });
}

/**
 * Host subcommands refuse to run inside a managed run container. The
 * heuristic: presence of the per-run manifest at ANTPATH_INDEX_PATH
 * (`/mnt/session/uploads/antpath/index.json`) means we're inside a
 * run and should expose `proxy`, not the platform-management verbs.
 *
 * Fails *closed* on read errors that are not "file not found": if the
 * manifest exists but is unreadable for any other reason (permissions,
 * IO error, etc.) we refuse to run the host verb rather than risk
 * leaking workspace-management calls into a sandboxed container.
 */
export async function refuseInsideManagedRun(io: CliIO, verb: string): Promise<boolean> {
  try {
    await io.readFile(ANTPATH_INDEX_PATH);
    io.stderr(
      `\`antpath ${verb}\` is a host command and cannot run inside a managed run container.\n` +
      "Use `antpath proxy ...` to call your declared upstream endpoints from inside the run.\n"
    );
    return true;
  } catch (err) {
    const code = (err as NodeJS.ErrnoException | undefined)?.code;
    if (code === "ENOENT" || code === "ENOTDIR") {
      // Manifest definitively absent; we are on a host. Allow the verb.
      return false;
    }
    io.stderr(
      `\`antpath ${verb}\` could not determine whether it is running inside a managed run ` +
      `container (error reading ${ANTPATH_INDEX_PATH}: ${(err as Error).message ?? "unknown"}). ` +
      `Refusing to proceed.\n`
    );
    return true;
  }
}

/**
 * Emit a JSON error body and return RUNTIME_ERR. The shape mirrors the
 * in-container proxy error envelope so a script-shaped consumer can
 * parse both surfaces the same way.
 */
export function emitJsonError(io: CliIO, code: string, message: string, extra: Record<string, unknown> = {}): CliExitCode {
  io.stderr(JSON.stringify({ error: code, message, ...extra }) + "\n");
  return RUNTIME_ERR;
}

/**
 * Repeatable `--var key=value` / `--mcp-secret name=value` /
 * `--proxy-auth name=value`-style flag parser. Returns the values in
 * insertion order (last-wins on duplicate keys).
 */
export function collectRepeatedKv(rest: readonly string[], flag: string): {
  readonly entries: Record<string, string>;
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const entries: Record<string, string> = {};
  const remaining: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const kv = rest[++i];
      if (kv === undefined) {
        return { entries, remaining, error: `${flag} requires a KEY=VALUE argument` };
      }
      const eq = kv.indexOf("=");
      if (eq <= 0) {
        return { entries, remaining, error: `${flag} must be in the form KEY=VALUE (got: ${kv})` };
      }
      entries[kv.slice(0, eq)] = kv.slice(eq + 1);
      continue;
    }
    remaining.push(arg);
  }
  return { entries, remaining, error: null };
}

/**
 * Repeatable `--flag <value>` collector. Each occurrence consumes one
 * argv slot. Returns the collected values in insertion order alongside
 * the remaining argv.
 */
export function collectRepeated(rest: readonly string[], flag: string): {
  readonly values: readonly string[];
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const values: string[] = [];
  const remaining: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const v = rest[++i];
      if (v === undefined) {
        return { values, remaining, error: `${flag} requires a value` };
      }
      values.push(v);
      continue;
    }
    remaining.push(arg);
  }
  return { values, remaining, error: null };
}

/**
 * Repeatable `--flag KEY=VALUE` collector that PRESERVES every
 * occurrence (no key-collision collapse). Use this when the same key
 * may legitimately appear multiple times — for example,
 * `--mcp-auth github=Authorization:Bearer t --mcp-auth github=X-Api-Key:k`
 * needs to register both headers, not just the last one.
 *
 * Returns `[key, value]` pairs in argv order alongside the remaining
 * argv with the consumed flags removed.
 */
export function collectRepeatedKvList(rest: readonly string[], flag: string): {
  readonly entries: ReadonlyArray<readonly [string, string]>;
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const entries: Array<readonly [string, string]> = [];
  const remaining: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const kv = rest[++i];
      if (kv === undefined) {
        return { entries, remaining, error: `${flag} requires a KEY=VALUE argument` };
      }
      const eq = kv.indexOf("=");
      if (eq <= 0) {
        return { entries, remaining, error: `${flag} must be in the form KEY=VALUE (got: ${kv})` };
      }
      entries.push([kv.slice(0, eq), kv.slice(eq + 1)] as const);
      continue;
    }
    remaining.push(arg);
  }
  return { entries, remaining, error: null };
}

export function takeFlagValue(
  rest: readonly string[],
  flag: string
): { value: string | null; remaining: readonly string[]; error: string | null } {
  let value: string | null = null;
  const remaining: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const v = rest[++i];
      if (v === undefined) {
        return { value, remaining, error: `${flag} requires a value` };
      }
      value = v;
      continue;
    }
    remaining.push(arg);
  }
  return { value, remaining, error: null };
}

/**
 * Parse a human-friendly duration into milliseconds. Accepts a bare
 * integer (interpreted as milliseconds) or `<number><unit>` where unit
 * is one of `ms`, `s`, `m`, `h` — e.g. `500ms`, `30s`, `8m`, `1h`.
 *
 * Returns the millisecond count, or an `error` string describing the
 * malformed input (the value is never silently coerced to 0). Rejects
 * negatives and non-finite values so a `--timeout` can never disable the
 * deadline by accident.
 */
export function parseDuration(input: string): { readonly ms: number | null; readonly error: string | null } {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/.exec(input.trim());
  if (!match) {
    return { ms: null, error: `invalid duration "${input}" (expected e.g. 500ms, 30s, 8m, 1h, or a bare ms integer)` };
  }
  const value = Number(match[1]);
  if (!Number.isFinite(value) || value < 0) {
    return { ms: null, error: `invalid duration "${input}" (must be a non-negative number)` };
  }
  const unit = match[2] ?? "ms";
  const factor = unit === "h" ? 3_600_000 : unit === "m" ? 60_000 : unit === "s" ? 1_000 : 1;
  return { ms: Math.round(value * factor), error: null };
}

export function takeBooleanFlag(rest: readonly string[], flag: string): {
  readonly present: boolean;
  readonly remaining: readonly string[];
} {
  const remaining: string[] = [];
  let present = false;
  for (const arg of rest) {
    if (arg === flag) {
      present = true;
      continue;
    }
    remaining.push(arg);
  }
  return { present, remaining };
}

/**
 * Take an option flag in either `--flag value` or `--flag=value` form.
 * Returns the trailing value (or undefined when the flag is absent)
 * plus the remaining args. Unlike `takeFlagValue`, this is permissive
 * about the `=` form which CLI users frequently expect.
 */
export function takeOptionFlag(
  rest: readonly string[],
  flag: string
): { readonly value: string | undefined; readonly remaining: readonly string[] } {
  const remaining: string[] = [];
  let value: string | undefined;
  const prefix = `${flag}=`;
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const next = rest[++i];
      if (next !== undefined) value = next;
      continue;
    }
    if (arg.startsWith(prefix)) {
      value = arg.slice(prefix.length);
      continue;
    }
    remaining.push(arg);
  }
  return { value, remaining };
}
