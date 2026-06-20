/**
 * `aex run` — submit a flat run via the dashboard BFF using the
 * same operations module the SDK uses.
 *
 * Two input modes (mutually exclusive):
 *
 *   1. `--config <path>` — plain run-request JSON:
 *      `{ model, system?, prompt, skills?, mcpServers?, environment?,
 *         runtimeSize?, timeout?, postHook?, proxyEndpoints?, metadata? }`. Skill
 *      entries use storage-neutral asset refs (`{ kind: "asset", ... }`).
 *      MCP entries may include `headers` — the CLI splits them into the
 *      `secrets.mcpServers` bag before posting.
 *
 *   2. Flat flags — for ad-hoc / scriptable invocations:
 *      --model <id>                          REQUIRED in flag mode
 *      --system @file | --system "literal"   optional system message
 *      --prompt @file | --prompt "literal"   REQUIRED in flag mode (repeatable)
 *      --mcp name=url                        MCP server (repeatable)
 *      --mcp-auth name=Header:Value          adds a header on the matching --mcp (repeatable)
 *      --metadata key=value                  string metadata entry (repeatable)
 *
 * Always required (both modes):
 *   --anthropic-api-key <key>      provider key (never stored)
 *   --api-token <token>            see `parseCommonHostFlags`
 *
 * Optional (both modes):
 *   --region <region>             product placement token (lhr, iad, sfo, bom); omitted infers/falls back
 *   --runtime-size <size>          managed runtime preset (e.g. shared-2x-2gb); default shared-1x-128mb
 *   --run-timeout <dur>            server-side run deadline (e.g. 1h); bounded [1m, 6h], default 1h
 *   --idempotency-key <key>        defaults to a fresh UUID
 *   --proxy-endpoint '<json>'      PlatformProxyEndpoint JSON (repeatable)
 *   --proxy-auth name=<spec>       bearer:tok | basic:u:p | header:v | query:v (repeatable)
 *   --follow                       poll events to stdout until terminal status
 *   --timeout <dur>                with --follow: give up after this long (e.g. 8m); exit code 3
 */
import {
  AEX_DEFAULT_BASE_URL,
  DEFAULT_RUN_PROVIDER,
  operations,
  parseRunRequestConfig,
  RUN_REGIONS,
  RUN_MODELS,
  RUNTIME_SIZES,
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  TERMINAL_RUN_STATUSES,
  validateProxyAuth,
  type RunRequestConfig,
  type McpServerRef,
  type PlatformRunSubmissionInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type PlatformProxyAuthValue,
  type PlatformProxyEndpoint,
  type PlatformProxyEndpointAuth,
  type RunModel,
  type RunProvider,
  type RunRegion,
  type RuntimeSize,
  type RuntimeKind,
  type SkillRef
} from "@aexhq/contracts";
import { resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  RUNTIME_ERR,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  collectRepeated,
  collectRepeatedKv,
  collectRepeatedKvList,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  takeBooleanFlag,
  takeFlagValue,
  takeOptionFlag
} from "./common.js";

// Membership-tested against the loose `string` run status from the BFF, so we
// back it with the canonical terminal set rather than a drift-prone local list.
const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

/* eslint-disable @typescript-eslint/no-unused-vars */ // AEX_DEFAULT_BASE_URL is re-exported only for assertion clarity.
void AEX_DEFAULT_BASE_URL;

export async function runRunCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "run")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  let rest = common.rest;

  const providerFlag = takeFlagValue(rest, "--provider");
  if (providerFlag.error) { io.stderr(`${providerFlag.error}\n`); return USAGE_ERR; }
  rest = providerFlag.remaining;
  let provider: RunProvider = DEFAULT_RUN_PROVIDER;
  if (providerFlag.value !== null) {
    if (!(RUN_PROVIDERS as readonly string[]).includes(providerFlag.value)) {
      io.stderr(`--provider must be one of: ${RUN_PROVIDERS.join(", ")} (got: ${providerFlag.value})\n`);
      return USAGE_ERR;
    }
    provider = providerFlag.value as RunProvider;
  }

  // Provider key flag handling: each provider in RUN_PROVIDERS has its
  // own `--<provider>-api-key` flag. Exactly the flag matching the
  // selected provider must be supplied; every other provider's flag
  // must be absent (mirrors the secrets-coupling rule the parser
  // re-enforces server-side).
  const providerKeyValues: Partial<Record<RunProvider, string>> = {};
  for (const p of RUN_PROVIDERS) {
    const flag = takeFlagValue(rest, `--${p}-api-key`);
    if (flag.error) { io.stderr(`${flag.error}\n`); return USAGE_ERR; }
    rest = flag.remaining;
    if (flag.value !== null) providerKeyValues[p] = flag.value;
  }
  if (!providerKeyValues[provider]) {
    io.stderr(`--${provider}-api-key is required when --provider is ${provider} (the platform does not store provider keys on your behalf)\n`);
    return USAGE_ERR;
  }
  for (const p of RUN_PROVIDERS) {
    if (p === provider) continue;
    if (providerKeyValues[p] !== undefined) {
      io.stderr(`--${p}-api-key is not allowed when --provider is ${provider}\n`);
      return USAGE_ERR;
    }
  }

  // Optional runtime selector. Validate the value against the wire enum
  // here so the user gets an early error from the CLI; the shared parser
  // re-runs the check when the request lands at the API plane.
  const runtimeFlag = takeFlagValue(rest, "--runtime");
  if (runtimeFlag.error) { io.stderr(`${runtimeFlag.error}\n`); return USAGE_ERR; }
  rest = runtimeFlag.remaining;
  let runtime: RuntimeKind | undefined;
  if (runtimeFlag.value !== null) {
    if (!(RUNTIME_KINDS as readonly string[]).includes(runtimeFlag.value)) {
      io.stderr(`--runtime must be one of: ${RUNTIME_KINDS.join(", ")} (got: ${runtimeFlag.value})\n`);
      return USAGE_ERR;
    }
    runtime = runtimeFlag.value as RuntimeKind;
  }

  const regionFlag = takeFlagValue(rest, "--region");
  if (regionFlag.error) { io.stderr(`${regionFlag.error}\n`); return USAGE_ERR; }
  rest = regionFlag.remaining;
  if (regionFlag.value && !(RUN_REGIONS as readonly string[]).includes(regionFlag.value)) {
    io.stderr(`--region must be one of: ${RUN_REGIONS.join(", ")}\n`);
    return USAGE_ERR;
  }

  const idempotency = takeFlagValue(rest, "--idempotency-key");
  if (idempotency.error) { io.stderr(`${idempotency.error}\n`); return USAGE_ERR; }
  rest = idempotency.remaining;

  // `--webhook <url>` sets an optional per-run callback URL. The shape
  // (https-only, no userinfo) is re-validated by the shared parser server-side;
  // here we only pass it through.
  const webhookFlag = takeFlagValue(rest, "--webhook");
  if (webhookFlag.error) { io.stderr(`${webhookFlag.error}\n`); return USAGE_ERR; }
  rest = webhookFlag.remaining;

  // `--runtime-size` selects a managed runtime size from the closed preset set.
  const runtimeSizeFlag = takeFlagValue(rest, "--runtime-size");
  if (runtimeSizeFlag.error) { io.stderr(`${runtimeSizeFlag.error}\n`); return USAGE_ERR; }
  rest = runtimeSizeFlag.remaining;
  if (runtimeSizeFlag.value && !(RUNTIME_SIZES as readonly string[]).includes(runtimeSizeFlag.value)) {
    io.stderr(`--runtime-size must be one of: ${RUNTIME_SIZES.join(", ")}\n`);
    return USAGE_ERR;
  }

  // `--run-timeout` is the SERVER-side run deadline (distinct from `--timeout`,
  // which bounds the client-side --follow loop). Format-checked locally for
  // fast feedback; the server applies the [1m, 6h] bounds + default.
  const runTimeoutFlag = takeFlagValue(rest, "--run-timeout");
  if (runTimeoutFlag.error) { io.stderr(`${runTimeoutFlag.error}\n`); return USAGE_ERR; }
  rest = runTimeoutFlag.remaining;
  if (runTimeoutFlag.value) {
    const parsed = parseDuration(runTimeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--run-timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
  }

  const follow = takeBooleanFlag(rest, "--follow");
  rest = follow.remaining;
  const timeoutFlag = takeOptionFlag(rest, "--timeout");
  rest = timeoutFlag.remaining;
  let followTimeoutMs: number | null = null;
  if (timeoutFlag.value !== undefined) {
    const parsed = parseDuration(timeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    followTimeoutMs = parsed.ms;
  }

  const config = takeFlagValue(rest, "--config");
  if (config.error) { io.stderr(`${config.error}\n`); return USAGE_ERR; }
  rest = config.remaining;

  // Flat flags
  const modelFlag = takeFlagValue(rest, "--model");
  if (modelFlag.error) { io.stderr(`${modelFlag.error}\n`); return USAGE_ERR; }
  rest = modelFlag.remaining;

  const systemFlag = takeFlagValue(rest, "--system");
  if (systemFlag.error) { io.stderr(`${systemFlag.error}\n`); return USAGE_ERR; }
  rest = systemFlag.remaining;

  const promptFlags = collectRepeated(rest, "--prompt");
  if (promptFlags.error) { io.stderr(`${promptFlags.error}\n`); return USAGE_ERR; }
  rest = promptFlags.remaining;

  const mcpFlags = collectRepeatedKv(rest, "--mcp");
  if (mcpFlags.error) { io.stderr(`${mcpFlags.error}\n`); return USAGE_ERR; }
  rest = mcpFlags.remaining;

  const mcpAuthFlags = collectRepeatedKvList(rest, "--mcp-auth");
  if (mcpAuthFlags.error) { io.stderr(`${mcpAuthFlags.error}\n`); return USAGE_ERR; }
  rest = mcpAuthFlags.remaining;

  const metadataFlags = collectRepeatedKv(rest, "--metadata");
  if (metadataFlags.error) { io.stderr(`${metadataFlags.error}\n`); return USAGE_ERR; }
  rest = metadataFlags.remaining;

  const proxyEndpointFlags = collectRepeated(rest, "--proxy-endpoint");
  if (proxyEndpointFlags.error) { io.stderr(`${proxyEndpointFlags.error}\n`); return USAGE_ERR; }
  rest = proxyEndpointFlags.remaining;

  const proxyAuthFlags = collectRepeatedKv(rest, "--proxy-auth");
  if (proxyAuthFlags.error) { io.stderr(`${proxyAuthFlags.error}\n`); return USAGE_ERR; }
  rest = proxyAuthFlags.remaining;

  const positional = rest.filter((a) => !a.startsWith("--"));
  const unknownFlags = rest.filter((a) => a.startsWith("--"));
  if (unknownFlags.length > 0) {
    io.stderr(`unknown flag: ${unknownFlags[0]}\n`);
    return USAGE_ERR;
  }
  if (positional.length > 0) {
    io.stderr(`aex run takes no positional arguments (got: ${positional.join(" ")})\n`);
    return USAGE_ERR;
  }

  // ---------------- Resolve run config (config XOR flat flags) ----------------
  let runConfig: RunRequestConfig;
  let mcpHeadersFromConfig: Map<string, Record<string, string>> = new Map();
  if (config.value) {
    if (modelFlag.value || systemFlag.value || promptFlags.values.length || Object.keys(mcpFlags.entries).length || Object.keys(metadataFlags.entries).length) {
      io.stderr("--config cannot be combined with --model/--system/--prompt/--mcp/--metadata\n");
      return USAGE_ERR;
    }
    try {
      const absPath = resolvePath(io.cwd(), config.value);
      const text = await io.readFile(absPath);
      const raw = JSON.parse(text) as unknown;
      const { mcpHeaders, normalised } = stripMcpHeadersForParsing(raw);
      mcpHeadersFromConfig = mcpHeaders;
      runConfig = parseRunRequestConfig(normalised);
    } catch (err) {
      io.stderr(`failed to load --config: ${(err as Error).message}\n`);
      return USAGE_ERR;
    }
  } else {
    if (!modelFlag.value) {
      io.stderr("--model is required when --config is not provided\n");
      return USAGE_ERR;
    }
    if (!(RUN_MODELS as readonly string[]).includes(modelFlag.value)) {
      io.stderr(`--model must be one of: ${RUN_MODELS.join(", ")} (got: ${modelFlag.value})\n`);
      return USAGE_ERR;
    }
    if (promptFlags.values.length === 0) {
      io.stderr("--prompt is required (repeatable)\n");
      return USAGE_ERR;
    }
    let resolvedPrompt: string[];
    try {
      resolvedPrompt = await Promise.all(promptFlags.values.map((v) => readMaybeFile(io, v)));
    } catch (err) {
      io.stderr(`failed to read --prompt file: ${(err as Error).message}\n`);
      return USAGE_ERR;
    }
    let resolvedSystem: string | undefined;
    if (systemFlag.value !== null) {
      try {
        resolvedSystem = await readMaybeFile(io, systemFlag.value);
      } catch (err) {
        io.stderr(`failed to read --system file: ${(err as Error).message}\n`);
        return USAGE_ERR;
      }
    }

    const mcpRefs: McpServerRef[] = [];
    for (const [name, url] of Object.entries(mcpFlags.entries)) {
      mcpRefs.push({ name, url });
    }

    runConfig = {
      model: modelFlag.value as RunModel,
      ...(resolvedSystem ? { system: resolvedSystem } : {}),
      prompt: resolvedPrompt,
      ...(mcpRefs.length > 0 ? { mcpServers: mcpRefs } : {}),
      ...(Object.keys(metadataFlags.entries).length > 0
        ? { metadata: { ...metadataFlags.entries } }
        : {})
    };
  }

  // ---------------- Resolve MCP secrets (--mcp-auth + config headers) ----------------
  const mcpServersForSubmission: McpServerRef[] = [...(runConfig.mcpServers ?? [])];
  const mcpServerSecrets: PlatformMcpServerSecret[] = [];
  const mcpHeaderBag = new Map<string, Record<string, string>>(mcpHeadersFromConfig);
  for (const [name, headerSpec] of mcpAuthFlags.entries) {
    const colon = headerSpec.indexOf(":");
    if (colon <= 0 || colon >= headerSpec.length - 1) {
      io.stderr(`--mcp-auth ${name}: expected 'HeaderName:Value' (got: ${headerSpec})\n`);
      return USAGE_ERR;
    }
    const headerName = headerSpec.slice(0, colon).trim();
    const headerValue = headerSpec.slice(colon + 1).trim();
    if (!headerName || !headerValue) {
      io.stderr(`--mcp-auth ${name}: header name and value must be non-empty\n`);
      return USAGE_ERR;
    }
    const existing = mcpHeaderBag.get(name) ?? {};
    if (Object.prototype.hasOwnProperty.call(existing, headerName)) {
      io.stderr(
        `--mcp-auth ${name}: duplicate header "${headerName}" — ` +
        `each header may be set only once per server\n`
      );
      return USAGE_ERR;
    }
    existing[headerName] = headerValue;
    mcpHeaderBag.set(name, existing);
  }

  for (const ref of mcpServersForSubmission) {
    const headers = mcpHeaderBag.get(ref.name);
    if (headers && Object.keys(headers).length > 0) {
      mcpServerSecrets.push({ name: ref.name, url: ref.url, headers });
    }
  }
  // Reject auth flags that reference an unknown MCP server.
  for (const name of mcpHeaderBag.keys()) {
    if (!mcpServersForSubmission.some((m) => m.name === name)) {
      io.stderr(`--mcp-auth ${name}: no matching --mcp / mcpServers entry declared\n`);
      return USAGE_ERR;
    }
  }

  // ---------------- Proxy endpoints + auth ----------------
  let proxyEndpoints: readonly PlatformProxyEndpoint[] = runConfig.proxyEndpoints ?? [];
  if (proxyEndpointFlags.values.length > 0) {
    try {
      proxyEndpoints = proxyEndpointFlags.values.map((raw, i) =>
        parseJsonOrThrow<PlatformProxyEndpoint>(raw, `--proxy-endpoint[${i}]`)
      );
    } catch (err) {
      io.stderr(`${(err as Error).message}\n`);
      return USAGE_ERR;
    }
  }
  const proxyAuth: PlatformProxyEndpointAuth[] = [];
  for (const [name, spec] of Object.entries(proxyAuthFlags.entries)) {
    const parsed = parseProxyAuth(spec);
    if (!parsed.ok) {
      io.stderr(`--proxy-auth ${name}: ${parsed.reason}\n`);
      return USAGE_ERR;
    }
    proxyAuth.push({ name, value: parsed.value });
  }
  if (proxyEndpoints.length > 0) {
    try {
      validateProxyAuth(proxyEndpoints, proxyAuth);
    } catch (err) {
      io.stderr(`proxy auth validation failed: ${(err as Error).message}\n`);
      return USAGE_ERR;
    }
  }

  // ---------------- Build submission ----------------
  const promptArray = Array.isArray(runConfig.prompt) ? [...runConfig.prompt] : [runConfig.prompt];
  const skills: SkillRef[] = runConfig.skills ? [...runConfig.skills] : [];
  const submission: PlatformSubmission = {
    model: runConfig.model,
    ...(runConfig.system ? { system: runConfig.system } : {}),
    prompt: promptArray,
    skills,
    agentsMd: [],
    files: [],
    mcpServers: mcpServersForSubmission,
    tools: [],
    ...(runConfig.environment ? { environment: runConfig.environment } : {}),
    ...(runConfig.metadata ? { metadata: runConfig.metadata } : {})
  };

  const secrets: PlatformInlineSecrets = {
    apiKey: providerKeyValues[provider] as string,
    ...(mcpServerSecrets.length > 0 ? { mcpServers: mcpServerSecrets } : {}),
    ...(proxyAuth.length > 0 ? { proxyEndpointAuth: proxyAuth } : {})
  };

  const request: PlatformRunSubmissionInput = {
    idempotencyKey: idempotency.value ?? generateIdempotencyKey(),
    provider,
    ...(runtime ? { runtime } : {}),
    submission,
    secrets,
    ...(regionFlag.value
      ? { region: regionFlag.value as RunRegion }
      : runConfig.region
        ? { region: runConfig.region }
        : {}),
    ...(runtimeSizeFlag.value
      ? { runtimeSize: runtimeSizeFlag.value as RuntimeSize }
      : runConfig.runtimeSize
        ? { runtimeSize: runConfig.runtimeSize }
        : {}),
    ...(runTimeoutFlag.value
      ? { timeout: runTimeoutFlag.value }
      : runConfig.timeout
        ? { timeout: runConfig.timeout }
        : {}),
    ...(runConfig.postHook ? { postHook: runConfig.postHook } : {}),
    ...(webhookFlag.value ? { webhook: { url: webhookFlag.value } } : {}),
    ...(proxyEndpoints.length > 0 ? { proxyEndpoints } : {})
  };

  const http = makeHttpClient(io, common.flags);
  let run;
  try {
    run = await operations.submitRun(http, request);
  } catch (err) {
    return emitJsonError(io, "submit_failed", (err as Error).message ?? "submission failed");
  }

  io.stdout(JSON.stringify(run) + "\n");
  if (!follow.present) return SUCCESS;

  let emittedEventCount = 0;
  let currentStatus = run.status;
  const deadline = followTimeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + followTimeoutMs;
  while (!TERMINAL_STATUSES.has(currentStatus)) {
    await sleep(2000);
    try {
      const events = await operations.listRunEvents(http, run.id);
      for (let i = emittedEventCount; i < events.length; i++) {
        io.stdout(JSON.stringify(events[i]) + "\n");
      }
      emittedEventCount = events.length;
    } catch (err) {
      io.stderr(`(transient) event poll failed: ${(err as Error).message}\n`);
    }
    try {
      const updated = await operations.getRun(http, run.id);
      currentStatus = updated.status;
    } catch (err) {
      io.stderr(`(transient) status poll failed: ${(err as Error).message}\n`);
    }
    if (!TERMINAL_STATUSES.has(currentStatus) && Date.now() >= deadline) {
      emitJsonError(io, "run_follow_timeout", `timed out after ${followTimeoutMs}ms following run`, { runId: run.id });
      return TIMEOUT_ERR;
    }
  }
  try {
    const final = await operations.getRun(http, run.id);
    io.stdout(JSON.stringify(final) + "\n");
    return final.status === "succeeded" ? SUCCESS : RUNTIME_ERR;
  } catch (err) {
    io.stderr(`final status fetch failed: ${(err as Error).message}\n`);
    return RUNTIME_ERR;
  }
}

/* ---------- helpers ---------- */

interface ProxyAuthOk { ok: true; value: PlatformProxyAuthValue; }
interface ProxyAuthErr { ok: false; reason: string; }

function parseProxyAuth(spec: string): ProxyAuthOk | ProxyAuthErr {
  const idx = spec.indexOf(":");
  if (idx <= 0) {
    return { ok: false, reason: `expected '<type>:<value>' (got: ${spec})` };
  }
  const type = spec.slice(0, idx);
  const restValue = spec.slice(idx + 1);
  switch (type) {
    case "bearer":
      if (!restValue) return { ok: false, reason: "bearer requires a token value" };
      return { ok: true, value: { type: "bearer", token: restValue } };
    case "header":
      if (!restValue) return { ok: false, reason: "header requires a value" };
      return { ok: true, value: { type: "header", value: restValue } };
    case "query":
      if (!restValue) return { ok: false, reason: "query requires a value" };
      return { ok: true, value: { type: "query", value: restValue } };
    case "basic": {
      const sep = restValue.indexOf(":");
      if (sep <= 0 || sep >= restValue.length - 1) {
        return { ok: false, reason: "basic requires <username>:<password>" };
      }
      return {
        ok: true,
        value: { type: "basic", username: restValue.slice(0, sep), password: restValue.slice(sep + 1) }
      };
    }
    default:
      return { ok: false, reason: `unknown auth type '${type}' (expected bearer|basic|header|query)` };
  }
}

function parseJsonOrThrow<T>(raw: string, label: string): T {
  try {
    return JSON.parse(raw) as T;
  } catch (err) {
    throw new Error(`${label} is not valid JSON: ${(err as Error).message}`);
  }
}

/**
 * Strip `headers` from each `mcpServers[i]` so the value can pass
 * `parseRunRequestConfig` (which forbids headers on the non-secret submission
 * shape). Returns the stripped object plus a name->headers map the
 * caller can fold into `secrets.mcpServers`.
 */
function stripMcpHeadersForParsing(input: unknown): {
  readonly normalised: unknown;
  readonly mcpHeaders: Map<string, Record<string, string>>;
} {
  const mcpHeaders = new Map<string, Record<string, string>>();
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    return { normalised: input, mcpHeaders };
  }
  const record = input as Record<string, unknown>;
  if (!Array.isArray(record.mcpServers)) {
    return { normalised: input, mcpHeaders };
  }
  const stripped = record.mcpServers.map((entry, i) => {
    if (!entry || typeof entry !== "object") return entry;
    const r = entry as Record<string, unknown>;
    if (r.headers && typeof r.headers === "object" && !Array.isArray(r.headers)) {
      const name = typeof r.name === "string" ? r.name : `__index_${i}__`;
      const headerObj: Record<string, string> = {};
      for (const [k, v] of Object.entries(r.headers as Record<string, unknown>)) {
        if (typeof v !== "string") {
          throw new Error(`mcpServers[${i}].headers["${k}"] must be a string`);
        }
        headerObj[k] = v;
      }
      mcpHeaders.set(name, headerObj);
      const { headers: _omit, ...rest } = r;
      void _omit;
      return rest;
    }
    return r;
  });
  return { normalised: { ...record, mcpServers: stripped }, mcpHeaders };
}

/**
 * Resolve a `--system` / `--prompt` argument that may be either a
 * literal string or a file reference. Conventions:
 *
 *   plain text          ->  the text itself
 *   "@path/to/file"     ->  contents of the file (relative to cwd)
 *   "@@literal"         ->  the literal "@literal" (escape for content
 *                            that genuinely starts with `@`)
 */
async function readMaybeFile(io: CliIO, value: string): Promise<string> {
  if (value.startsWith("@@")) {
    return value.slice(1);
  }
  if (value.startsWith("@")) {
    const path = resolvePath(io.cwd(), value.slice(1));
    return io.readFile(path);
  }
  return value;
}

function generateIdempotencyKey(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (c?.randomUUID) return c.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

