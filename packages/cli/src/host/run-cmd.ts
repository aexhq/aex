/**
 * `aex run` — one-shot over the session API, a THIN pass-through over the SDK's
 * `Aex` client (dependency-inverted: the CLI constructs `new Aex(...)` and calls
 * `submit()` rather than re-implementing create+send over the raw `operations`
 * layer). This makes capability + validation single-sourced in the SDK, so the
 * CLI physically cannot drift from it:
 *   - model / provider / run-timeout are arbitrated by the SDK (no CLI-local
 *     `RUN_MODELS` gate, no `parseDuration`-only floor check); an unknown model
 *     with an explicit `--provider` is forward-compat accepted, a typo yields a
 *     shared "did you mean?" hint.
 *   - skills / tools / agentsMd / files ATTACH via `--skill`/`--tool`/
 *     `--agents-md`/`--file`, mapped to SDK `Skill`/`Tool`/`AgentsMd`/`File`
 *     instances the SDK prepares + uploads.
 *
 * Two input modes (mutually exclusive):
 *   1. `--config <path>` — run-request JSON `{ model, system?, prompt,
 *      mcpServers?, environment?, runtimeSize?, timeout?, metadata? }`.
 *   2. Flat flags — `--model`, `--system @file|text`, `--prompt @file|text`
 *      (repeatable), `--mcp name=url`, `--mcp-auth name=Hdr:Val`,
 *      `--metadata key=value`.
 *
 * Always required: `--<provider>-api-key <key>` for the resolved provider, and
 * the common `--api-key <token>`.
 */
import {
  parseRunRequestConfig,
  parseRunTimeout,
  providersForModel,
  resolveModelProvider,
  RUNTIME_SIZES,
  RUN_PROVIDERS,
  type JsonValue,
  type PlatformEnvironment,
  type RunModel,
  type RunProvider,
  type RuntimeSize
} from "@aexhq/contracts";
import {
  Aex,
  AgentsMd,
  File as AexFile,
  McpServer,
  Skill,
  Tool,
  type SessionEnvironmentOptions,
  type SessionRunOptions
} from "@aexhq/sdk";
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
  describeApiError,
  emitJsonError,
  isSessionOk,
  resolveCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  suggest,
  takeBooleanFlag,
  takeFlagValue,
  takeOptionFlag
} from "./common.js";

/** Default idle window a one-shot session may sit before the platform reaps it. Mirrors the SDK. */
const DEFAULT_SESSION_IDLE_TTL = "3m";

export async function runRunCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "run")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  let rest = common.rest;

  const providerFlag = takeFlagValue(rest, "--provider");
  if (providerFlag.error) { io.stderr(`${providerFlag.error}\n`); return USAGE_ERR; }
  rest = providerFlag.remaining;
  let explicitProvider: RunProvider | undefined;
  if (providerFlag.value !== null) {
    if (!(RUN_PROVIDERS as readonly string[]).includes(providerFlag.value)) {
      const hint = suggest(providerFlag.value, RUN_PROVIDERS);
      io.stderr(
        `--provider must be one of: ${RUN_PROVIDERS.join(", ")} (got: ${providerFlag.value})` +
          `${hint ? `; did you mean "${hint}"?` : ""}\n`
      );
      return USAGE_ERR;
    }
    explicitProvider = providerFlag.value as RunProvider;
  }

  // Provider key flags: `--<provider>-api-key`. The selected provider's key is
  // REQUIRED; extra providers' keys ride along for subagents (server-side
  // inheritance). Collected into `apiKeys`; the SDK re-validates the required key.
  const providerKeyValues: Partial<Record<RunProvider, string>> = {};
  for (const p of RUN_PROVIDERS) {
    const flag = takeFlagValue(rest, `--${p}-api-key`);
    if (flag.error) { io.stderr(`${flag.error}\n`); return USAGE_ERR; }
    rest = flag.remaining;
    if (flag.value !== null) providerKeyValues[p] = flag.value;
  }

  const idempotency = takeFlagValue(rest, "--idempotency-key");
  if (idempotency.error) { io.stderr(`${idempotency.error}\n`); return USAGE_ERR; }
  rest = idempotency.remaining;

  const webhookFlag = takeFlagValue(rest, "--webhook");
  if (webhookFlag.error) { io.stderr(`${webhookFlag.error}\n`); return USAGE_ERR; }
  rest = webhookFlag.remaining;

  const runtimeSizeFlag = takeFlagValue(rest, "--runtime-size");
  if (runtimeSizeFlag.error) { io.stderr(`${runtimeSizeFlag.error}\n`); return USAGE_ERR; }
  rest = runtimeSizeFlag.remaining;
  if (runtimeSizeFlag.value && !(RUNTIME_SIZES as readonly string[]).includes(runtimeSizeFlag.value)) {
    const hint = suggest(runtimeSizeFlag.value, RUNTIME_SIZES);
    io.stderr(`--runtime-size must be one of: ${RUNTIME_SIZES.join(", ")}${hint ? `; did you mean "${hint}"?` : ""}\n`);
    return USAGE_ERR;
  }

  // `--run-timeout` is the SERVER-side run deadline. Validated client-side with
  // the SAME SSoT parser the SDK uses (`parseRunTimeout`: format AND the [1m,8h]
  // floor), synchronously, BEFORE any network call — no `parseDuration`-only
  // format check that lets a sub-1m value die async server-side.
  const runTimeoutFlag = takeFlagValue(rest, "--run-timeout");
  if (runTimeoutFlag.error) { io.stderr(`${runTimeoutFlag.error}\n`); return USAGE_ERR; }
  rest = runTimeoutFlag.remaining;
  if (runTimeoutFlag.value) {
    try {
      parseRunTimeout(runTimeoutFlag.value);
    } catch (err) {
      io.stderr(`--run-timeout: ${(err as Error).message}\n`);
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

  const modelFlag = takeFlagValue(rest, "--model");
  if (modelFlag.error) { io.stderr(`${modelFlag.error}\n`); return USAGE_ERR; }
  rest = modelFlag.remaining;

  const systemFlag = takeFlagValue(rest, "--system");
  if (systemFlag.error) { io.stderr(`${systemFlag.error}\n`); return USAGE_ERR; }
  rest = systemFlag.remaining;

  const promptFlags = collectRepeated(rest, "--prompt");
  if (promptFlags.error) { io.stderr(`${promptFlags.error}\n`); return USAGE_ERR; }
  rest = promptFlags.remaining;

  // Attach flags (T6a): repeatable @-file references mapped to SDK primitives.
  const skillFlags = collectRepeated(rest, "--skill");
  if (skillFlags.error) { io.stderr(`${skillFlags.error}\n`); return USAGE_ERR; }
  rest = skillFlags.remaining;
  const toolFlags = collectRepeated(rest, "--tool");
  if (toolFlags.error) { io.stderr(`${toolFlags.error}\n`); return USAGE_ERR; }
  rest = toolFlags.remaining;
  const agentsMdFlags = collectRepeated(rest, "--agents-md");
  if (agentsMdFlags.error) { io.stderr(`${agentsMdFlags.error}\n`); return USAGE_ERR; }
  rest = agentsMdFlags.remaining;
  const fileFlags = collectRepeated(rest, "--file");
  if (fileFlags.error) { io.stderr(`${fileFlags.error}\n`); return USAGE_ERR; }
  rest = fileFlags.remaining;

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
  if (proxyEndpointFlags.values.length > 0 || Object.keys(proxyAuthFlags.entries).length > 0) {
    io.stderr("--proxy-endpoint and --proxy-auth are no longer supported; make HTTP calls from your code and pass credentials via secrets.\n");
    return USAGE_ERR;
  }

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
  let model: RunModel;
  let system: string | undefined;
  let promptArray: string[];
  let configMcpServers: readonly { readonly name: string; readonly url: string }[] = [];
  let configEnvironment: PlatformEnvironment | undefined;
  let configRuntimeSize: RuntimeSize | undefined;
  let configTimeout: string | undefined;
  let metadata: Record<string, JsonValue> | undefined;
  const mcpHeaderBag = new Map<string, Record<string, string>>();

  if (config.value) {
    if (modelFlag.value || systemFlag.value || promptFlags.values.length || Object.keys(mcpFlags.entries).length || Object.keys(metadataFlags.entries).length) {
      io.stderr("--config cannot be combined with --model/--system/--prompt/--mcp/--metadata\n");
      return USAGE_ERR;
    }
    let runConfig;
    try {
      const absPath = resolvePath(io.cwd(), config.value);
      const text = await io.readFile(absPath);
      const raw = JSON.parse(text) as unknown;
      const { mcpHeaders, normalised } = stripMcpHeadersForParsing(raw);
      for (const [name, headers] of mcpHeaders) mcpHeaderBag.set(name, headers);
      runConfig = parseRunRequestConfig(normalised);
    } catch (err) {
      io.stderr(`failed to load --config: ${(err as Error).message}\n`);
      return USAGE_ERR;
    }
    model = runConfig.model;
    system = runConfig.system;
    promptArray = Array.isArray(runConfig.prompt) ? [...runConfig.prompt] : [runConfig.prompt];
    configMcpServers = runConfig.mcpServers ?? [];
    configEnvironment = runConfig.environment;
    configRuntimeSize = runConfig.runtimeSize;
    configTimeout = runConfig.timeout;
    metadata = runConfig.metadata ? { ...runConfig.metadata } : undefined;
  } else {
    if (!modelFlag.value) {
      io.stderr("--model is required when --config is not provided\n");
      return USAGE_ERR;
    }
    // No CLI-local RUN_MODELS gate: the SDK arbitrates model/provider policy
    // (below), forward-compat with an explicit --provider.
    model = modelFlag.value as RunModel;
    if (promptFlags.values.length === 0) {
      io.stderr("--prompt is required (repeatable)\n");
      return USAGE_ERR;
    }
    try {
      promptArray = await Promise.all(promptFlags.values.map((v) => readMaybeFile(io, v)));
    } catch (err) {
      io.stderr(`failed to read --prompt file: ${(err as Error).message}\n`);
      return USAGE_ERR;
    }
    if (systemFlag.value !== null) {
      try {
        system = await readMaybeFile(io, systemFlag.value);
      } catch (err) {
        io.stderr(`failed to read --system file: ${(err as Error).message}\n`);
        return USAGE_ERR;
      }
    }
    configMcpServers = Object.entries(mcpFlags.entries).map(([name, url]) => ({ name, url }));
    metadata = Object.keys(metadataFlags.entries).length > 0 ? { ...metadataFlags.entries } : undefined;
  }

  // ---------------- Model → provider policy (shared SSoT: did-you-mean) --------
  let provider: RunProvider;
  try {
    provider = resolveModelProvider(model, explicitProvider);
  } catch (err) {
    io.stderr(`--model: ${(err as Error).message}\n`);
    return USAGE_ERR;
  }
  if (!providerKeyValues[provider]) {
    const inferred = explicitProvider === undefined && providersForModel(model).length > 0;
    io.stderr(
      `--${provider}-api-key is required for provider ${provider}` +
        `${inferred ? ` (inferred from --model ${model})` : ""}` +
        ` (the platform does not store provider keys on your behalf)\n`
    );
    return USAGE_ERR;
  }

  // ---------------- Resolve MCP auth headers (--mcp-auth + config headers) ------
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
        `--mcp-auth ${name}: duplicate header "${headerName}" — each header may be set only once per server\n`
      );
      return USAGE_ERR;
    }
    existing[headerName] = headerValue;
    mcpHeaderBag.set(name, existing);
  }
  for (const name of mcpHeaderBag.keys()) {
    if (!configMcpServers.some((m) => m.name === name)) {
      io.stderr(`--mcp-auth ${name}: no matching --mcp / mcpServers entry declared\n`);
      return USAGE_ERR;
    }
  }
  const mcpServers = configMcpServers.map((m) => {
    const headers = mcpHeaderBag.get(m.name);
    return McpServer.remote({
      name: m.name,
      url: m.url,
      ...(headers && Object.keys(headers).length > 0 ? { headers } : {})
    });
  });

  // ---------------- Build the SDK attach primitives (T6a) ----------------------
  let skills: Skill[];
  let tools: Tool[];
  let agentsMd: AgentsMd[];
  let files: AexFile[];
  try {
    skills = await Promise.all(skillFlags.values.map((ref) => buildSkill(io, ref)));
    tools = await Promise.all(toolFlags.values.map((ref) => buildTool(io, ref)));
    agentsMd = await Promise.all(agentsMdFlags.values.map((ref) => buildAgentsMd(io, ref)));
    files = await Promise.all(fileFlags.values.map((ref) => buildFile(io, ref)));
  } catch (err) {
    io.stderr(`failed to attach asset: ${(err as Error).message}\n`);
    return USAGE_ERR;
  }

  const environment = toSessionEnvironment(configEnvironment);
  const runtimeSize = (runtimeSizeFlag.value as RuntimeSize | null) ?? configRuntimeSize;
  const timeout = runTimeoutFlag.value ?? configTimeout;

  const options: SessionRunOptions = {
    message: promptArray,
    provider,
    model,
    ...(system ? { system } : {}),
    ...(skills.length > 0 ? { skills } : {}),
    ...(tools.length > 0 ? { tools } : {}),
    ...(agentsMd.length > 0 ? { agentsMd } : {}),
    ...(files.length > 0 ? { files } : {}),
    ...(mcpServers.length > 0 ? { mcpServers } : {}),
    ...(metadata ? { metadata } : {}),
    apiKeys: providerKeyValues,
    ...(environment ? { environment } : {}),
    ...(runtimeSize ? { runtime: runtimeSize } : {}),
    overrides: {
      idleTtl: DEFAULT_SESSION_IDLE_TTL,
      ...(timeout ? { timeout } : {})
    },
    ...(webhookFlag.value ? { webhook: { url: webhookFlag.value } } : {}),
    ...(idempotency.value ? { idempotencyKey: idempotency.value } : {})
  };

  const aex = new Aex({ baseUrl: common.flags.aexUrl, apiKey: common.flags.apiKey, fetch: io.fetchImpl });

  // Fire-and-forget submit: create the session + POST the first turn in one
  // call, then print the accepted session record (the SDK owns the wire shape).
  let session;
  try {
    const submitted = await aex.submit(options);
    session = submitted.session;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "run_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }

  io.stdout(JSON.stringify(session.record) + "\n");
  if (!follow.present) return SUCCESS;

  // `--follow`: stream the turn's events (NDJSON) until the session parks, then
  // print the final record. Streaming rides the SDK's polling event accessor.
  const controller = new AbortController();
  let timedOut = false;
  const timer =
    followTimeoutMs === null
      ? null
      : setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, followTimeoutMs);
  try {
    for await (const event of session.events().stream({ signal: controller.signal })) {
      io.stdout(JSON.stringify(event) + "\n");
    }
  } catch (err) {
    io.stderr(`(transient) event stream failed: ${(err as Error).message}\n`);
  } finally {
    if (timer) clearTimeout(timer);
  }

  if (timedOut) {
    emitJsonError(io, "run_follow_timeout", `timed out after ${followTimeoutMs}ms following session`, {
      sessionId: session.id,
      hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
    });
    return TIMEOUT_ERR;
  }

  try {
    const final = await session.refresh();
    io.stdout(JSON.stringify(final) + "\n");
    if (isSessionOk(final.status)) return SUCCESS;
    io.stderr(
      JSON.stringify({
        error: "session_not_ok",
        sessionId: session.id,
        status: final.status,
        hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
      }) + "\n"
    );
    return RUNTIME_ERR;
  } catch (err) {
    io.stderr(`final status fetch failed: ${(err as Error).message}\n`);
    return RUNTIME_ERR;
  }
}

/* ---------- attach-primitive builders ---------- */

async function buildSkill(io: CliIO, ref: string): Promise<Skill> {
  const content = await readAtFile(io, ref);
  return Skill.fromContent(content);
}

async function buildTool(io: CliIO, ref: string): Promise<Tool> {
  const content = await readAtFile(io, ref);
  const entry = baseName(stripAt(ref));
  const name = deriveName(ref, 1);
  // A single-file `--tool @x.js` has no declared arg schema; default to an
  // open object so the SDK's authoring-time entry/manifest guard is satisfied.
  return Tool.fromFiles({
    name,
    description: `Custom tool ${name}`,
    entry,
    inputSchema: { type: "object", properties: {}, additionalProperties: true },
    files: { [entry]: content }
  });
}

async function buildAgentsMd(io: CliIO, ref: string): Promise<AgentsMd> {
  const content = await readAtFile(io, ref);
  return AgentsMd.fromContent(content, { name: deriveName(ref, 2) });
}

async function buildFile(io: CliIO, ref: string): Promise<AexFile> {
  const content = await readAtFile(io, ref);
  const name = baseName(stripAt(ref));
  return AexFile.fromBytes({ name, bytes: new TextEncoder().encode(content) });
}

/* ---------- helpers ---------- */

/** Read a `@path` (or bare path) attach reference via the injected IO. */
async function readAtFile(io: CliIO, value: string): Promise<string> {
  const path = resolvePath(io.cwd(), stripAt(value));
  return io.readFile(path);
}

function stripAt(value: string): string {
  return value.startsWith("@") ? value.slice(1) : value;
}

function baseName(p: string): string {
  const trimmed = p.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}

/**
 * Derive a lowercase, hyphen-safe workspace/tool name from a file reference.
 * `minLen` pads short slugs so a name still satisfies the 2+-char workspace
 * pattern (agentsMd); the factory revalidates and throws on a truly bad name.
 */
function deriveName(ref: string, minLen: number): string {
  const base = baseName(stripAt(ref));
  const noExt = base.includes(".") ? base.slice(0, base.lastIndexOf(".")) : base;
  let slug = noExt.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  while (slug.length < minLen) slug += "x";
  return slug;
}

/** Map a run-config `PlatformEnvironment` onto the SDK's `SessionEnvironmentOptions`. */
function toSessionEnvironment(env: PlatformEnvironment | undefined): SessionEnvironmentOptions | undefined {
  if (!env) return undefined;
  const out: SessionEnvironmentOptions = {
    ...(env.networking ? { networking: env.networking } : {}),
    ...(env.packages ? { packages: env.packages } : {}),
    ...(env.envVars ? { variables: env.envVars } : {})
  };
  return Object.keys(out).length === 0 ? undefined : out;
}

/**
 * Strip `headers` from each `mcpServers[i]` so the value can pass
 * `parseRunRequestConfig` (which forbids headers on the non-secret submission
 * shape). Returns the stripped object plus a name->headers map.
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
 * Resolve a `--system` / `--prompt` argument that may be a literal or a file
 * reference: plain text → itself; `@path` → file contents; `@@literal` → the
 * literal `@literal`.
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
