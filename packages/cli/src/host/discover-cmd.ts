/**
 * Discoverability list commands (DX2): `aex models|providers|tools|runtime-sizes list`.
 *
 * Pure reads of the closed `@aexhq/contracts` SSoT sets — zero drift, no token,
 * no network — so they work on a host AND inside a managed run container (like
 * `--help`). Default output is a fixed-width human table; `--json` emits the raw
 * array for scripting.
 */
import {
  BUILTIN_TOOL_NAMES,
  DEFAULT_BUILTIN_TOOLS,
  DEFAULT_RUNTIME_SIZE,
  PROVIDER_PUBLIC_SUPPORT,
  RUN_MODELS,
  RUN_MODELS_BY_PROVIDER,
  RUN_PROVIDERS,
  RUNTIME_SIZES,
  RUNTIME_SIZE_PRESETS,
  providerForModel,
  providersForModel,
  type RunProvider
} from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import { type CliExitCode, SUCCESS, USAGE_ERR, takeBooleanFlag } from "./common.js";

const DEFAULT_BUILTIN_SET = new Set<string>(DEFAULT_BUILTIN_TOOLS);

/** Render a left-aligned fixed-width table (header + rows) to stdout. */
function renderTable(io: CliIO, headers: readonly string[], rows: readonly (readonly string[])[]): void {
  const widths = headers.map((h, col) =>
    Math.max(h.length, ...rows.map((r) => (r[col] ?? "").length))
  );
  const line = (cells: readonly string[]): string =>
    cells.map((c, col) => (c ?? "").padEnd(widths[col]!)).join("  ").replace(/\s+$/, "");
  io.stdout(line(headers) + "\n");
  io.stdout(widths.map((w) => "-".repeat(w)).join("  ").replace(/\s+$/, "") + "\n");
  for (const row of rows) io.stdout(line(row) + "\n");
}

function wantsJson(argv: readonly string[]): { json: boolean; rest: readonly string[] } {
  const { present, remaining } = takeBooleanFlag(argv, "--json");
  return { json: present, rest: remaining };
}

export function runModelsCmd(io: CliIO, argv: readonly string[]): CliExitCode {
  const { json, rest } = wantsJson(stripListSubcommand(argv));
  if (hasUnknown(io, rest, "models")) return USAGE_ERR;
  const entries = RUN_MODELS.map((model) => ({
    model,
    defaultProvider: providerForModel(model) ?? null,
    providers: providersForModel(model)
  }));
  if (json) {
    io.stdout(JSON.stringify(entries) + "\n");
  } else {
    renderTable(
      io,
      ["MODEL", "DEFAULT PROVIDER", "PROVIDERS"],
      entries.map((e) => [e.model, e.defaultProvider ?? "-", e.providers.join(", ")])
    );
  }
  return SUCCESS;
}

export function runProvidersCmd(io: CliIO, argv: readonly string[]): CliExitCode {
  const { json, rest } = wantsJson(stripListSubcommand(argv));
  if (hasUnknown(io, rest, "providers")) return USAGE_ERR;
  const entries = RUN_PROVIDERS.map((provider) => ({
    provider,
    displayName: PROVIDER_PUBLIC_SUPPORT[provider as RunProvider].displayName,
    models: RUN_MODELS_BY_PROVIDER[provider as RunProvider] ?? []
  }));
  if (json) {
    io.stdout(JSON.stringify(entries) + "\n");
  } else {
    renderTable(
      io,
      ["PROVIDER", "NAME", "MODELS"],
      entries.map((e) => [e.provider, e.displayName, e.models.join(", ")])
    );
  }
  return SUCCESS;
}

export function runToolsCmd(io: CliIO, argv: readonly string[]): CliExitCode {
  const { json, rest } = wantsJson(stripListSubcommand(argv));
  if (hasUnknown(io, rest, "tools")) return USAGE_ERR;
  const entries = BUILTIN_TOOL_NAMES.map((name) => ({
    tool: name,
    default: DEFAULT_BUILTIN_SET.has(name)
  }));
  if (json) {
    io.stdout(JSON.stringify(entries) + "\n");
  } else {
    renderTable(
      io,
      ["TOOL", "DEFAULT"],
      entries.map((e) => [e.tool, e.default ? "yes" : "opt-in"])
    );
  }
  return SUCCESS;
}

export function runRuntimeSizesCmd(io: CliIO, argv: readonly string[]): CliExitCode {
  const { json, rest } = wantsJson(stripListSubcommand(argv));
  if (hasUnknown(io, rest, "runtime-sizes")) return USAGE_ERR;
  const entries = RUNTIME_SIZES.map((size) => ({
    size,
    cpus: RUNTIME_SIZE_PRESETS[size].cpus,
    memoryMb: RUNTIME_SIZE_PRESETS[size].memoryMb,
    default: size === DEFAULT_RUNTIME_SIZE
  }));
  if (json) {
    io.stdout(JSON.stringify(entries) + "\n");
  } else {
    renderTable(
      io,
      ["SIZE", "CPUS", "MEMORY (MB)", "DEFAULT"],
      entries.map((e) => [e.size, String(e.cpus), String(e.memoryMb), e.default ? "yes" : ""])
    );
  }
  return SUCCESS;
}

/** Drop an optional leading `list` subcommand so `models list` == `models`. */
function stripListSubcommand(argv: readonly string[]): readonly string[] {
  return argv[0] === "list" ? argv.slice(1) : argv;
}

/** Reject stray positional args (a typo'd subcommand) with a clear error. */
function hasUnknown(io: CliIO, rest: readonly string[], verb: string): boolean {
  const stray = rest.filter((a) => !a.startsWith("--"));
  if (stray.length > 0) {
    io.stderr(`usage: aex ${verb} list [--json]\n`);
    io.stderr(`unexpected arguments: ${stray.join(" ")}\n`);
    return true;
  }
  return false;
}
