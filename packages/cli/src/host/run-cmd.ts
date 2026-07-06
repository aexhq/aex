/**
 * `aex run` — one-shot over the session API. Host CLI parsing stays thin and
 * submits through the shared public contracts transport (`operations.submit`):
 *   - model / provider / run-timeout are arbitrated by shared contracts (no CLI-local
 *     `RUN_MODELS` gate, no `parseDuration`-only floor check); an unknown model
 *     with an explicit `--provider` is forward-compat accepted, a typo yields a
 *     shared "did you mean?" hint.
 *   - skills / tools / agentsMd / files ATTACH via `--skill`/`--tool`/
 *     `--agents-md`/`--file`, staged through the same public asset protocol the
 *     SDK uses.
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
  operations,
  parseRunRequestConfig,
  parseRunTimeout,
  providersForModel,
  resolveModelProvider,
  RUNTIME_SIZES,
  RUN_PROVIDERS,
  type HttpClient,
  type JsonValue,
  type PlatformEnvironment,
  type RunModel,
  type RunProvider,
  type Session,
  type RuntimeSize
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
  describeApiError,
  emitJsonError,
  isSessionOk,
  isSessionParked,
  makeHttpClient,
  resolveCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  suggest,
  takeBooleanFlag,
  takeFlagValue,
  takeOptionFlag
} from "./common.js";
import {
  buildCliAgentsMd,
  buildCliFile,
  buildCliSkill,
  buildCliTool,
  submitCliRun,
  toCliSessionEnvironment,
  type CliAgentsMdDraft,
  type CliFileDraft,
  type CliMcpServer,
  type CliRunSubmitOptions,
  type CliSkillDraft,
  type CliToolDraft
} from "./run-submit.js";
import { openEnvelopeStream } from "./stream-render.js";

/** Default idle window a one-shot session may sit before the platform reaps it. Mirrors the SDK. */
const DEFAULT_SESSION_IDLE_TTL = "3m";
const FOLLOW_SETTLE_POLL_DEADLINE_MS = 60_000;
const FOLLOW_SETTLE_POLL_INTERVAL_MS = 750;

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
  const mcpServers: CliMcpServer[] = configMcpServers.map((m) => {
    const headers = mcpHeaderBag.get(m.name);
    return {
      name: m.name,
      url: m.url,
      ...(headers && Object.keys(headers).length > 0 ? { headers } : {})
    };
  });

  // ---------------- Build the SDK attach primitives (T6a) ----------------------
  let skills: CliSkillDraft[];
  let tools: CliToolDraft[];
  let agentsMd: CliAgentsMdDraft[];
  let files: CliFileDraft[];
  try {
    skills = await Promise.all(skillFlags.values.map((ref) => buildSkill(io, ref)));
    tools = await Promise.all(toolFlags.values.map((ref) => buildTool(io, ref)));
    agentsMd = await Promise.all(agentsMdFlags.values.map((ref) => buildAgentsMd(io, ref)));
    files = await Promise.all(fileFlags.values.map((ref) => buildFile(io, ref)));
  } catch (err) {
    io.stderr(`failed to attach asset: ${(err as Error).message}\n`);
    return USAGE_ERR;
  }

  const environment = toCliSessionEnvironment(configEnvironment);
  const runtimeSize = (runtimeSizeFlag.value as RuntimeSize | null) ?? configRuntimeSize;
  const timeout = runTimeoutFlag.value ?? configTimeout;

  const options: CliRunSubmitOptions = {
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

  const http = makeHttpClient(io, common.flags);

  if (follow.present && !io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex run --follow` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun."
      }) + "\n"
    );
    return USAGE_ERR;
  }

  // Fire-and-forget submit: create the session, post the first turn, then print
  // the accepted session record.
  let session;
  try {
    session = await submitCliRun(http, io.fetchImpl, options);
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "run_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }

  io.stdout(JSON.stringify(session) + "\n");
  if (!follow.present) return SUCCESS;

  // `--follow`: stream the live coordinator envelopes as NDJSON until the
  // session parks, then print the final session record.
  const controller = new AbortController();
  let timedOut = false;
  let interrupted = false;
  io.onSignal?.("SIGINT", () => {
    interrupted = true;
    controller.abort();
  });
  const timer =
    followTimeoutMs === null
      ? null
      : setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, followTimeoutMs);
  let lastSeq = -1;
  try {
    const stream = openEnvelopeStream(io, http, session.id, {
      signal: controller.signal,
      ...(common.flags.debug ? { debug: (line: string) => io.stderr(`[aex] ${line}\n`) } : {})
    });
    for await (const event of stream) {
      lastSeq = event.sequence;
      io.stdout(JSON.stringify(event) + "\n");
    }
  } catch (err) {
    if (timer) clearTimeout(timer);
    if (timedOut) {
      emitJsonError(io, "run_follow_timeout", `timed out after ${followTimeoutMs}ms following session`, {
        sessionId: session.id,
        lastSeq,
        ...followTimeoutContext(session),
        hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
      });
      return TIMEOUT_ERR;
    }
    if (interrupted) {
      io.stderr(`(interrupted) followed up to seq ${lastSeq}\n`);
      return SUCCESS;
    }
    io.stderr(`(transient) event stream failed: ${(err as Error).message}\n`);
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "run_follow_timeout", `timed out after ${followTimeoutMs}ms following session`, {
      sessionId: session.id,
      lastSeq,
      ...followTimeoutContext(session),
      hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
    });
    return TIMEOUT_ERR;
  }
  if (interrupted) {
    io.stderr(`(interrupted) followed up to seq ${lastSeq}\n`);
    return SUCCESS;
  }

  try {
    const final = await followSettledSessionRecord(http, session.id, controller.signal);
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

async function followSettledSessionRecord(http: HttpClient, sessionId: string, signal: AbortSignal): Promise<Session> {
  const deadline = Date.now() + FOLLOW_SETTLE_POLL_DEADLINE_MS;
  let last = await operations.getSession(http, sessionId);
  if (isSessionParked(last.status)) return last;

  while (signal.aborted !== true && Date.now() < deadline) {
    try {
      await sleep(FOLLOW_SETTLE_POLL_INTERVAL_MS, signal);
    } catch {
      return last;
    }
    const next = await operations.getSession(http, sessionId).catch(() => undefined);
    if (next === undefined) continue;
    last = next;
    if (isSessionParked(next.status)) return next;
  }
  return last;
}

async function sleep(ms: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return;
  await new Promise<void>((resolve, reject) => {
    const cleanup = (): void => signal.removeEventListener("abort", abort);
    const timer = setTimeout(() => {
      cleanup();
      resolve();
    }, ms);
    const abort = (): void => {
      clearTimeout(timer);
      cleanup();
      reject(new Error("aborted"));
    };
    signal.addEventListener("abort", abort, { once: true });
  });
}

/* ---------- attach-primitive builders ---------- */

async function buildSkill(io: CliIO, ref: string): Promise<CliSkillDraft> {
  const content = await readAtFile(io, ref);
  return buildCliSkill(content, ref);
}

async function buildTool(io: CliIO, ref: string): Promise<CliToolDraft> {
  const content = await readAtFile(io, ref);
  const entry = baseName(stripAt(ref));
  const name = deriveName(ref, 1);
  return buildCliTool({
    name,
    description: `Custom tool ${name}`,
    entry,
    content
  });
}

async function buildAgentsMd(io: CliIO, ref: string): Promise<CliAgentsMdDraft> {
  const content = await readAtFile(io, ref);
  return buildCliAgentsMd(content, deriveName(ref, 2));
}

async function buildFile(io: CliIO, ref: string): Promise<CliFileDraft> {
  const content = await readAtFile(io, ref);
  const name = baseName(stripAt(ref));
  return buildCliFile({ name, content });
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

function followTimeoutContext(session: {
  readonly status?: unknown;
  readonly turnSeq?: unknown;
  readonly turnStatus?: unknown;
}): Record<string, string | number> {
  return {
    ...(typeof session.status === "string" ? { sessionStatus: session.status } : {}),
    ...(typeof session.turnSeq === "number" ? { turnSeq: session.turnSeq } : {}),
    ...(typeof session.turnStatus === "string" ? { turnStatus: session.turnStatus } : {})
  };
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
