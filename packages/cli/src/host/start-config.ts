import {
  parseModelSlug,
  parseSessionRequestConfig,
  type JsonValue,
  type ModelName,
  type PlatformEnvironment,
  type RuntimeSize
} from "@aexhq/contracts";
import { resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import type { StartArguments } from "./start-arguments.js";
import type { CliMcpServer } from "./start-submit.js";
import { startValidationMessage } from "./start-validation.js";

export interface ResolvedStartConfig {
  readonly model: ModelName;
  readonly system?: string;
  readonly message: readonly string[];
  readonly mcpServers: readonly CliMcpServer[];
  readonly environment?: PlatformEnvironment;
  readonly configRuntimeSize?: RuntimeSize;
  readonly configTimeout?: string;
  readonly metadata?: Record<string, JsonValue>;
}

export type ResolvedStartConfigResult =
  | { readonly ok: true; readonly value: ResolvedStartConfig }
  | { readonly ok: false; readonly error: string };

/** Resolve config/flat input and MCP secrets before attachments. */
export async function resolveStartConfig(
  io: CliIO,
  args: StartArguments
): Promise<ResolvedStartConfigResult> {
  let model: ModelName;
  let system: string | undefined;
  let message: string[];
  let configMcpServers: readonly { readonly name: string; readonly url: string }[] = [];
  let environment: PlatformEnvironment | undefined;
  let configRuntimeSize: RuntimeSize | undefined;
  let configTimeout: string | undefined;
  let metadata: Record<string, JsonValue> | undefined;
  const mcpHeaderBag = new Map<string, Record<string, string>>();

  if (args.configPath) {
    if (
      args.model || args.system || args.prompts.length ||
      Object.keys(args.mcpEntries).length || Object.keys(args.metadataEntries).length
    ) {
      return {
        ok: false,
        error: startValidationMessage(
          "--config",
          "cannot be combined with --model/--system/--prompt/--mcp/--metadata"
        )
      };
    }
    let sessionConfig;
    try {
      const absPath = resolvePath(io.cwd(), args.configPath);
      const text = await io.readFile(absPath);
      const raw = JSON.parse(text) as unknown;
      const { mcpHeaders, normalised } = stripMcpHeadersForParsing(raw);
      for (const [name, headers] of mcpHeaders) mcpHeaderBag.set(name, headers);
      sessionConfig = parseSessionRequestConfig(normalised);
    } catch (err) {
      return { ok: false, error: startValidationMessage("--config", `failed to load: ${(err as Error).message}`) };
    }
    model = sessionConfig.model;
    system = sessionConfig.system;
    message = Array.isArray(sessionConfig.prompt) ? [...sessionConfig.prompt] : [sessionConfig.prompt];
    configMcpServers = sessionConfig.mcpServers ?? [];
    environment = sessionConfig.environment;
    configRuntimeSize = sessionConfig.runtimeSize;
    configTimeout = sessionConfig.timeout;
    metadata = sessionConfig.metadata ? { ...sessionConfig.metadata } : undefined;
  } else {
    if (!args.model) {
      return { ok: false, error: startValidationMessage("--model", "is required when --config is not provided") };
    }
    // Shape-check the slug HERE, in the same tier as the missing-flag case above, so
    // `--model` reports consistently: a missing value and a malformed one are both
    // usage errors (exit 2, plain line). The gateway cutover left slug parsing to the
    // submission builder, whose failures surface as a `session_failed` envelope with
    // exit 1 — which made the commonest migration mistake (a bare model name instead
    // of creator/model) look like a run that had started and failed. An unknown but
    // WELL-FORMED slug still passes: the gateway owns the catalog, not the CLI.
    try {
      parseModelSlug(args.model, "--model");
    } catch (err) {
      return { ok: false, error: startValidationMessage("--model", err instanceof Error ? err.message : String(err)) };
    }
    model = args.model as ModelName;
    if (args.prompts.length === 0) return { ok: false, error: startValidationMessage("--prompt", "is required (repeatable)") };
    try {
      message = await Promise.all(args.prompts.map((value) => readMaybeFile(io, value)));
    } catch (err) {
      return { ok: false, error: startValidationMessage("--prompt", `failed to read file: ${(err as Error).message}`) };
    }
    if (args.system !== undefined) {
      try {
        system = await readMaybeFile(io, args.system);
      } catch (err) {
        return { ok: false, error: startValidationMessage("--system", `failed to read file: ${(err as Error).message}`) };
      }
    }
    configMcpServers = Object.entries(args.mcpEntries).map(([name, url]) => ({ name, url }));
    metadata = Object.keys(args.metadataEntries).length > 0 ? { ...args.metadataEntries } : undefined;
  }

  for (const [name, headerSpec] of args.mcpAuthEntries) {
    const colon = headerSpec.indexOf(":");
    if (colon <= 0 || colon >= headerSpec.length - 1) {
      return {
        ok: false,
        error: startValidationMessage("--mcp-auth", `${name}: expected 'HeaderName:Value' (got: ${headerSpec})`)
      };
    }
    const headerName = headerSpec.slice(0, colon).trim();
    const headerValue = headerSpec.slice(colon + 1).trim();
    if (!headerName || !headerValue) {
      return { ok: false, error: startValidationMessage("--mcp-auth", `${name}: header name and value must be non-empty`) };
    }
    const existing = mcpHeaderBag.get(name) ?? {};
    if (Object.prototype.hasOwnProperty.call(existing, headerName)) {
      return {
        ok: false,
        error: startValidationMessage(
          "--mcp-auth",
          `${name}: duplicate header "${headerName}" — each header may be set only once per server`
        )
      };
    }
    existing[headerName] = headerValue;
    mcpHeaderBag.set(name, existing);
  }
  for (const name of mcpHeaderBag.keys()) {
    if (!configMcpServers.some((server) => server.name === name)) {
      return {
        ok: false,
        error: startValidationMessage("--mcp-auth", `${name}: no matching --mcp / mcpServers entry declared`)
      };
    }
  }
  const mcpServers: CliMcpServer[] = configMcpServers.map((server) => {
    const headers = mcpHeaderBag.get(server.name);
    return {
      name: server.name,
      url: server.url,
      ...(headers && Object.keys(headers).length > 0 ? { headers } : {})
    };
  });

  return {
    ok: true,
    value: {
      model,
      ...(system !== undefined ? { system } : {}),
      message,
      mcpServers,
      ...(environment ? { environment } : {}),
      ...(configRuntimeSize !== undefined ? { configRuntimeSize } : {}),
      ...(configTimeout !== undefined ? { configTimeout } : {}),
      ...(metadata ? { metadata } : {})
    }
  };
}

function stripMcpHeadersForParsing(input: unknown): {
  readonly normalised: unknown;
  readonly mcpHeaders: Map<string, Record<string, string>>;
} {
  const mcpHeaders = new Map<string, Record<string, string>>();
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    return { normalised: input, mcpHeaders };
  }
  const record = input as Record<string, unknown>;
  if (!Array.isArray(record.mcpServers)) return { normalised: input, mcpHeaders };
  const stripped = record.mcpServers.map((entry, index) => {
    if (!entry || typeof entry !== "object") return entry;
    const server = entry as Record<string, unknown>;
    if (server.headers && typeof server.headers === "object" && !Array.isArray(server.headers)) {
      const name = typeof server.name === "string" ? server.name : `__index_${index}__`;
      const headers: Record<string, string> = {};
      for (const [key, value] of Object.entries(server.headers as Record<string, unknown>)) {
        if (typeof value !== "string") {
          throw new Error(`mcpServers[${index}].headers["${key}"] must be a string`);
        }
        headers[key] = value;
      }
      mcpHeaders.set(name, headers);
      const { headers: _omit, ...rest } = server;
      void _omit;
      return rest;
    }
    return server;
  });
  return { normalised: { ...record, mcpServers: stripped }, mcpHeaders };
}

async function readMaybeFile(io: CliIO, value: string): Promise<string> {
  if (value.startsWith("@@")) return value.slice(1);
  if (value.startsWith("@")) return io.readFile(resolvePath(io.cwd(), value.slice(1)));
  return value;
}
