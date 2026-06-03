import {
  AntpathError,
  DEFAULT_CREDENTIAL_MODE,
  DEFAULT_RUN_PROVIDER,
  HttpClient,
  RUNTIME_KINDS,
  RunStateError,
  operations,
  parseCredentialMode,
  streamCoordinatorEvents,
  type AntpathEvent,
  type AgentsMdRecord,
  type CredentialMode,
  type AgentsMdRef,
  type DebugSink,
  type FetchLike,
  type FileRecord,
  type FileRef,
  type McpServerRef,
  type Output,
  type PlatformRunSubmissionInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type PlatformProxyEndpoint,
  type PlatformProxyEndpointAuth,
  type Run,
  type RunEvent,
  type RunProvider,
  type RunUnit,
  type RuntimeSize,
  type RuntimeKind,
  type SignedOutputLink,
  type Skill as SkillRecord,
  type SkillRef,
  type WhoAmI,
  TERMINAL_RUN_STATUSES
} from "@antpath/contracts";
import { uploadAsset } from "./asset-upload.js";
import { AgentsMd } from "./agents-md.js";
import { File } from "./file.js";
import { McpServer } from "./mcp-server.js";
import { ProxyEndpoint, splitProxyEndpoints } from "./proxy-endpoint.js";
import { Skill } from "./skill.js";

export interface AntpathClientOptions {
  /** Workspace-scoped SDK API token. */
  readonly apiToken: string;
  /**
   * API plane root, e.g. `https://antpath.example.com`. Optional —
   * defaults to the canonical hosted URL (`https://api.antpath.ai`).
   * Override for local, staging, or other hosted antpath API planes.
   */
  readonly baseUrl?: string;
  /** Optional `fetch` override for testing. */
  readonly fetch?: FetchLike;
  /**
   * Local debug output. `true` prints a redacted one-line trace per HTTP
   * request to stderr (method, path, status, elapsed); pass a function to
   * route the traces elsewhere. Purely local — nothing is uploaded.
   */
  readonly debug?: boolean | DebugSink;
}

/**
 * Per-run submission options. Everything the user wants to send is
 * spelled out at the call site:
 *
 *   - `model` / `system` / `prompt` — the agent's brief.
 *   - `skills` — array of local `Skill` instances
 *     (`Skill.fromFiles` / `Skill.fromPath`). Local skills are materialized
 *     to the hosted asset store before the run lands.
 *   - `mcpServers` — array of `McpServer` instances (headers split into
 *     `secrets.mcpServers` server-side; the public submission only
 *     carries `{ name, url }`).
 *   - `proxyEndpoints` — array of `ProxyEndpoint` instances. The auth
 *     secret is bundled into the constructor and split into
 *     `secrets.proxyEndpointAuth` server-side; the public submission
 *     only carries the declaration (`{ name, baseUrl, authShape, … }`).
 *   - `secrets.<provider>.apiKey` — REQUIRED for the selected provider.
 *     The platform never holds a long-lived provider key on your behalf.
 *
 * `idempotencyKey` is auto-generated when omitted; pass one explicitly
 * if you want client-driven retry safety across process restarts.
 */
export interface SubmitRunOptions {
  /**
   * Credential source for upstream provider access. Omitted defaults to
   * `"byok"`, which requires `secrets.<provider>.apiKey` as today.
   * `"managed"` is reserved for paid managed-key mode and currently fails
   * closed until the service is available.
   */
  readonly credentialMode?: CredentialMode;
  /**
   * Provider selector. Optional — defaults to
   * {@link DEFAULT_RUN_PROVIDER} (`"anthropic"`). The call site must
   * supply the matching `secrets.<provider>.apiKey` and MUST NOT
   * supply any other provider's secret block.
   */
  readonly provider?: RunProvider;
  /**
   * Optional runtime selector. Omit it or pass `"managed"`; both run on
   * the managed runtime through the hosted BYOK provider-proxy. `"native"`
   * is no longer accepted.
   */
  readonly runtime?: RuntimeKind;
  readonly model: string;
  readonly system?: string;
  readonly prompt: string | readonly string[];
  readonly skills?: readonly Skill[];
  readonly agentsMd?: readonly AgentsMd[];
  readonly files?: readonly File[];
  readonly mcpServers?: readonly McpServer[];
  readonly environment?: PlatformSubmission["environment"];
  readonly metadata?: PlatformSubmission["metadata"];
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens.
   * Prefer the {@link RuntimeSizes} symbol const, e.g.
   * `RuntimeSizes.SHARED_2X_2GB`.
   */
  readonly runtimeSize?: RuntimeSize;
  /**
   * Run deadline as a duration string (`"1h"`, `"90m"`, `"30s"`). Bounded to
   * [1m, 6h]; omit for the 1h default. Applies to both runtimes.
   */
  readonly timeout?: string;
  readonly proxyEndpoints?: readonly ProxyEndpoint[];
  /**
   * Container paths to capture as output objects at session terminal.
   *
   * - Omitted: the managed runtime default output directory is captured
   *   (`/workspace/outputs`).
   * - Present: the listed paths override the runtime default. Captured bytes
   *   land in private storage and can be retrieved via `client.outputs(runId)` /
   *   `client.download(runId)`.
   *
   * Paths are absolute UNIX paths (start with `/`), max 32 entries,
   * max 512 bytes per entry, no `..` segments, no NUL bytes. See
   * `packages/sdk/docs/outputs.md` for the full contract.
   */
  readonly outputDirs?: readonly string[];
  /**
   * Override the managed runtime builtin extensions enabled inside the runner.
   *
   * - Omitted (default): the runner enables `["developer"]` which gives
   *   the agent `shell`, `write`, `edit`, and `tree` tools (bash, grep
   *   via shell, file read via shell or editor, file edit).
   * - Empty array: the agent runs with zero builtin extensions —
   *   useful for pure-MCP setups where every tool comes from a
   *   submitted `mcpServers` entry.
   * - Custom list: e.g. `["developer", "computercontroller"]` to add
   *   web search alongside the default shell/edit toolkit.
   *
   * Validation: each entry matches `/^[a-z][a-z0-9_-]{0,63}$/`, max 16
   * entries, deduplicated server-side.
   */
  readonly builtins?: readonly string[];
  readonly secrets: PlatformInlineSecrets;
  readonly idempotencyKey?: string;
  readonly signal?: AbortSignal;
}

export interface StreamEventsOptions {
  /** Poll interval in ms for the `RunEvent` snapshot loop. Default 1000. */
  readonly intervalMs?: number;
  readonly signal?: AbortSignal;
}

export interface StreamEnvelopesOptions {
  /** Starting cursor — events with `sequence >= from` are delivered. Default 0. */
  readonly from?: number;
  readonly signal?: AbortSignal;
}

export interface WaitForRunOptions {
  readonly intervalMs?: number;
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
}

export type OutputFilePathMatch = "exact" | "suffix";

export interface OutputFilePathSelector {
  readonly path: string;
  readonly match?: OutputFilePathMatch;
}

export interface OutputFileIdSelector {
  readonly id: string;
}

export type OutputFileSelector = Output | OutputFileIdSelector | OutputFilePathSelector;

export interface OutputDownloadOptions {
  readonly to?: string;
}

/**
 * One captured debug artifact returned by {@link AntpathClient.getRunDebugLogs}.
 *
 *   - `filename` is the path under `runs/{runId}/logs/` — leading
 *     `runtime/` for runtime logs and `host/` for host logs.
 *   - `text` is populated when the content type looks textual
 *     (`text/*`, `application/json`); decoded as UTF-8.
 *   - `bytesBase64` is always present so a caller that wants raw bytes
 *     (e.g. piping into a file) can use it uniformly across textual
 *     and binary artifacts.
 */
export interface RunDebugLog {
  readonly filename: string;
  readonly sizeBytes: number;
  readonly contentType: string;
  readonly createdAt: string;
  readonly text?: string;
  readonly bytesBase64: string;
}

export interface RunDebugLogError {
  readonly filename: string;
  readonly message: string;
}

export interface RunDebugLogs {
  readonly runId: string;
  readonly logs: ReadonlyArray<RunDebugLog>;
  readonly errors: ReadonlyArray<RunDebugLogError>;
}

/**
 * Workspace skill admin operations exposed under `client.skills`.
 *
 * New run submissions usually use `Skill.fromFiles(...)` or
 * `Skill.fromPath(...)` directly inside `submitRun`; the SDK materializes
 * those bytes to the hosted asset store before the run lands. This namespace is the read/delete
 * surface for workspace skill records and the internal transport used by the
 * legacy CLI upload command.
 */
export class SkillsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly SkillRecord[]> {
    return operations.listSkills(this.#http) as Promise<readonly SkillRecord[]>;
  }

  get(skillId: string): Promise<SkillRecord> {
    return operations.getSkill(this.#http, skillId);
  }

  delete(skillId: string): Promise<void> {
    return operations.deleteSkill(this.#http, skillId);
  }

  /**
   * Lookup a live workspace skill by `(name, contentHash)`.
   *
   * Returns the matching `Skill` record or `null` when no live row
   * carries that hash. The `contentHash` is the wire format
   * `sha256:<hex>` returned by `hashSkillBundle` (and stored verbatim
   * on every skill row). The hash space is unique enough that one
   * row at most can match, so this is a single keyed lookup.
   *
   * Consumers can call this directly when they already have a hash in hand
   * and want to know whether the skill is already persisted.
   */
  findByHash(args: { readonly name: string; readonly contentHash: string }): Promise<SkillRecord | null> {
    return operations.findSkillByHash(this.#http, args);
  }

  /**
   * Lookup a live workspace skill by `name`. Returns the matching
   * `Skill` record or `null` when no live row carries that name.
   * Implemented as a list-and-filter against the existing `/api/skills`
   * endpoint — typical workspace skill counts are small enough that
   * the cost is negligible.
   */
  findByName(name: string): Promise<SkillRecord | null> {
    return operations.findSkillByName(this.#http, name);
  }

  /**
   * Internal: post a pre-bundled skill zip to the BFF. Only
   * `Skill.upload` calls this. NOT part of the public API.
   */
  async _uploadSkillBundle(args: { readonly name: string; readonly body: Uint8Array }): Promise<SkillRecord> {
    return operations.createSkillBundle(this.#http, {
      name: args.name,
      body: args.body,
      contentType: "application/zip",
      filename: `${args.name}.zip`
    });
  }
}

/**
 * Workspace AgentsMd admin operations exposed under `client.agentsMd`.
 *
 * New run submissions usually use `AgentsMd.fromContent(...)` or
 * `AgentsMd.fromPath(...)` directly inside `submitRun`; the SDK
 * materializes those bytes to the hosted asset store before the run lands. This namespace is
 * the read/delete surface for persisted AgentsMd records plus an internal
 * upload transport retained for legacy callers.
 */
export class AgentsMdClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly AgentsMdRecord[]> {
    return operations.listAgentsMd(this.#http);
  }

  get(agentsMdId: string): Promise<AgentsMdRecord> {
    return operations.getAgentsMd(this.#http, agentsMdId);
  }

  delete(agentsMdId: string): Promise<void> {
    return operations.deleteAgentsMd(this.#http, agentsMdId);
  }

  /**
   * Internal: post an AgentsMd markdown string to the BFF.
   * NOT part of the public API.
   */
  async _uploadAgentsMd(args: {
    readonly name: string;
    readonly content: string;
  }): Promise<AgentsMdRecord> {
    return operations.createAgentsMd(this.#http, args);
  }
}

/**
 * Workspace File admin operations exposed under `client.files`.
 *
 * New run submissions usually use `File.fromPath(...)` or
 * `File.fromBytes(...)` directly inside `submitRun`; the SDK materializes
 * those bytes to the hosted asset store before the run lands. This namespace is the read/delete
 * surface for persisted file records plus an internal upload transport
 * retained for legacy callers.
 */
export class FilesClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(): Promise<readonly FileRecord[]> {
    return operations.listFiles(this.#http);
  }

  get(fileId: string): Promise<FileRecord> {
    return operations.getFile(this.#http, fileId);
  }

  delete(fileId: string): Promise<void> {
    return operations.deleteFile(this.#http, fileId);
  }

  /**
   * Internal: post a pre-bundled file zip to the BFF.
   * NOT part of the public API.
   */
  async _uploadFile(args: { readonly name: string; readonly bytes: Uint8Array }): Promise<FileRecord> {
    return operations.createFile(this.#http, args);
  }
}

/**
 * Unified user-facing client for the antpath platform. The same class
 * powers the published `antpath` SDK and (under the hood) every host-side
 * subcommand of the in-container `antpath` CLI. All operations talk to
 * the dashboard BFF and operate on durable run records.
 *
 * The SDK never asks the caller for a workspace id — workspace identity
 * is derived server-side from the API token on every request. Use
 * `client.whoami()` if you want to introspect which workspace the
 * token resolves to.
 */
export class AntpathClient {
  readonly #http: HttpClient;
  readonly skills: SkillsClient;
  readonly agentsMd: AgentsMdClient;
  readonly files: FilesClient;

  constructor(options: AntpathClientOptions) {
    if (!options.apiToken) {
      throw new Error("AntpathClient: apiToken is required");
    }
    this.#http = new HttpClient({
      ...(options.baseUrl ? { baseUrl: options.baseUrl } : {}),
      apiToken: options.apiToken,
      ...(options.fetch ? { fetch: options.fetch } : {}),
      // Opt-in local diagnostics: emit a redacted per-request trace to
      // stderr. Uploads nothing. A caller wanting a custom sink can pass
      // a function instead of `true`.
      ...(options.debug
        ? { debug: typeof options.debug === "function" ? options.debug : (line: string) => console.error(line) }
        : {})
    });
    this.skills = new SkillsClient(this.#http);
    this.agentsMd = new AgentsMdClient(this.#http);
    this.files = new FilesClient(this.#http);
  }

  /**
   * Internal: forwards to `SkillsClient._uploadSkillBundle`. NOT part of
   * the public API.
   *
   * NOTE (tech-debt): this is part of the legacy workspace-skill upload
   * surface (`SkillsClient` + `operations.createSkillBundle` + the TUS
   * chunked path in asset-upload.ts). The live submit path materializes
   * inline skills via `uploadAsset` instead; `Skill` no longer
   * exposes `.upload()`/`.fromId()`. This surface is retained pending a
   * deliberate deprecation pass (it still threads into the CLI host
   * commands), tracked in the remediation plan as item 4a.
   */
  async _uploadSkillBundle(args: { readonly name: string; readonly body: Uint8Array }): Promise<SkillRecord> {
    return this.skills._uploadSkillBundle(args);
  }

  /**
   * Internal: an `AgentsMd.upload(this)` shortcut that bypasses
   * `client.agentsMd` indirection. Forwarded to
   * `AgentsMdClient._uploadAgentsMd`. NOT part of the public API.
   */
  async _uploadAgentsMd(args: {
    readonly name: string;
    readonly content: string;
  }): Promise<AgentsMdRecord> {
    return this.agentsMd._uploadAgentsMd(args);
  }

  /**
   * Internal: a `File.upload(this)` shortcut that bypasses
   * `client.files` indirection. Forwarded to
   * `FilesClient._uploadFile`. NOT part of the public API.
   */
  async _uploadFile(args: { readonly name: string; readonly bytes: Uint8Array }): Promise<FileRecord> {
    return this.files._uploadFile(args);
  }

  /**
   * Submit a run and wait for it to reach a terminal state. Returns the
   * final `Run` record. For long-running flows, prefer `submitRun` +
   * `stream(runId)` + `wait(runId)`.
   */
  async run(options: SubmitRunOptions): Promise<Run> {
    const runId = await this.submitRun(options);
    return this.waitForRun(runId, options.signal ? { signal: options.signal } : {});
  }

  /**
   * Submit a run and return its run id immediately. Use that id with
   * `wait`, `stream`, `outputs`, `download`, `cancel`, or `delete`.
   *
   * The SDK splits `mcpServers[i].headers` into `secrets.mcpServers`
   * and `proxyEndpoints[i]` auth values into `secrets.proxyEndpointAuth`
   * before sending so credentials never enter the hashed submission or
   * the run snapshot.
   *
   * Unstaged inline skills (`Skill.fromFiles` / `Skill.fromPath`
   * without a prior `.upload`) are accepted: the SDK switches to a
   * multipart body that carries the canonical zip bytes alongside the
   * JSON submission. The dashboard BFF ingests each one through the
   * standard workspace-skill upload pipeline (dedup by content hash;
   * upload via the existing two-phase pending → ready flow) and
   * rewrites the run's `skills[]` to reference the resulting `skl_*`
   * ids. The bytes persist on antpath; the user can browse and
   * download the resulting workspace skill from the dashboard.
   */
  async submitRun(options: SubmitRunOptions): Promise<string> {
    if (!options || typeof options !== "object") {
      throw new Error("AntpathClient.submitRun: options is required");
    }
    const provider: RunProvider = options.provider ?? DEFAULT_RUN_PROVIDER;
    const credentialMode = parseCredentialMode(options.credentialMode);
    if (credentialMode === "managed") {
      throw new AntpathError(
        "CREDENTIAL_INVALID",
        "AntpathClient.submitRun: credentialMode \"managed\" is not available"
      );
    }
    if (!options.secrets) {
      throw new Error("AntpathClient.submitRun: secrets is required");
    }
    // The matching provider's apiKey is required; every OTHER provider's
    // secret block must be absent. The shared parser re-runs this check
    // on the server; failing early here gives the caller a synchronous
    // error before any network call.
    const providerSecret = (options.secrets as Record<string, { apiKey?: string } | undefined>)[provider];
    if (!providerSecret?.apiKey) {
      throw new Error(`AntpathClient.submitRun: secrets.${provider}.apiKey is required`);
    }
    for (const other of ["anthropic", "deepseek", "openai", "gemini", "mistral"] as const) {
      if (other === provider) continue;
      if ((options.secrets as Record<string, unknown>)[other] !== undefined) {
        throw new Error(
          `AntpathClient.submitRun: secrets.${other} is not allowed when provider is ${provider}`
        );
      }
    }
    if (typeof options.model !== "string" || !options.model) {
      throw new Error("AntpathClient.submitRun: model is required");
    }
    const prompt = normalisePrompt(options.prompt);
    const { endpoints: proxyEndpointDeclarations, auth: proxyEndpointAuthFromInstances } =
      splitProxyEndpoints(options.proxyEndpoints ?? []);
    const mergedProxyAuth = mergeProxyEndpointAuth(
      proxyEndpointAuthFromInstances,
      options.secrets.proxyEndpointAuth ?? []
    );

    // Walk Skill / AgentsMd / File instances and materialize every draft before
    // the submit round-trip. The wire shape carries only kind:"asset" refs.
    const assetSkills = await materializeSkills(this.#http, options.skills ?? []);
    const assetAgentsMd = await materializeAgentsMd(this.#http, options.agentsMd ?? []);
    const assetFiles = await materializeFiles(this.#http, options.files ?? []);
    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      options.secrets.mcpServers ?? []
    );

    const submission: PlatformSubmission = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      prompt,
      skills: assetSkills,
      agentsMd: assetAgentsMd,
      files: assetFiles,
      // submissionMcpServers may contain workspace refs of the shape
      // {kind:"workspace", id:"mcp_..."}. The BFF runs
      // `resolveWorkspaceMcpRefsInSubmission` BEFORE the shared parser
      // and replaces them with the resolved {name, url}, so by the
      // time anything reads PlatformSubmission post-parse the
      // shape matches McpServerRef. The cast acknowledges that the
      // SDK is producing pre-resolution wire input here.
      mcpServers: submissionMcpServers as readonly McpServerRef[],
      ...(options.environment ? { environment: options.environment } : {}),
      ...(options.metadata ? { metadata: options.metadata } : {}),
      ...(options.outputDirs && options.outputDirs.length > 0
        ? { outputDirs: options.outputDirs }
        : {}),
      // Pass-through `builtins` verbatim — including an empty array,
      // which is the "disable all builtins" signal. Distinguish from
      // omitted (default applies) via `!== undefined`.
      ...(options.builtins !== undefined ? { builtins: options.builtins } : {})
    };

    const secrets: PlatformInlineSecrets = {
      ...options.secrets,
      ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {}),
      ...(mergedProxyAuth.length > 0 ? { proxyEndpointAuth: mergedProxyAuth } : {})
    };

    const request: PlatformRunSubmissionInput = {
      idempotencyKey: options.idempotencyKey ?? generateIdempotencyKey(),
      // Always include `provider` on the wire so dashboard / proxy
      // tooling never has to second-guess what the runtime saw. The
      // shared parser still defaults to `anthropic` when callers omit
      // the field entirely, but the SDK has resolved it by here.
      provider,
      ...(credentialMode !== DEFAULT_CREDENTIAL_MODE ? { credentialMode } : {}),
      // `runtime` is optional on the wire — absent means let the
      // dispatcher auto-route. Only emit it when the caller asked for
      // a specific runtime so the wire shape stays minimal.
      ...(options.runtime ? { runtime: options.runtime } : {}),
      submission,
      ...(options.runtimeSize ? { runtimeSize: options.runtimeSize } : {}),
      ...(options.timeout ? { timeout: options.timeout } : {}),
      secrets,
      ...(proxyEndpointDeclarations.length > 0
        ? { proxyEndpoints: proxyEndpointDeclarations }
        : {})
    };

    if (
      options.runtime !== undefined &&
      !(RUNTIME_KINDS as readonly string[]).includes(options.runtime)
    ) {
      throw new AntpathError(
        "RUNTIME_UNSUPPORTED",
        `AntpathClient.submitRun: runtime must be one of: ${RUNTIME_KINDS.join(", ")} ` +
          `(got ${JSON.stringify(options.runtime)})`
      );
    }

    // All inline refs were materialized above, so submitRun is
    // always a plain JSON post. The multipart code path is gone.
    const run = await operations.submitRun(this.#http, request);
    return run.id;
  }

  getRun(runId: string): Promise<Run> {
    return operations.getRun(this.#http, runId);
  }

  /** Short alias for `getRun`. */
  get(runId: string): Promise<Run> {
    return this.getRun(runId);
  }

  /**
   * Fetch the self-contained `RunUnit`: parsed submission inputs,
   * attempts, indexed events (inline + cursor for the tail), raw
   * provider-event Storage manifest, outputs, capture failures,
   * proxy-call audit, pinned workspace skills, provider skills,
   * inline skills. Backed by the same endpoint as `getRun` but
   * typed against the full wire shape — use this when you need
   * fields beyond `{id, status, timestamps, usage}`.
   */
  getRunUnit(runId: string): Promise<RunUnit> {
    return operations.getRunUnit(this.#http, runId);
  }

  /** Short alias for `getRunUnit`. */
  getUnit(runId: string): Promise<RunUnit> {
    return this.getRunUnit(runId);
  }

  listEvents(runId: string): Promise<readonly RunEvent[]> {
    return operations.listRunEvents(this.#http, runId);
  }

  /** Short alias for `listEvents`. */
  events(runId: string): Promise<readonly RunEvent[]> {
    return this.listEvents(runId);
  }

  /**
   * Yield run events (the `RunEvent` snapshot shape) as they arrive, by
   * polling the coordinator-backed `/events` endpoint until the run reaches
   * a terminal state, the signal aborts, or the caller breaks the iterator.
   *
   * For the live, low-latency envelope stream prefer {@link streamEnvelopes}
   * (coordinator WebSocket). This polling form stays for consumers that want
   * the loose `RunEvent` shape without a WS.
   */
  async *streamEvents(runId: string, options: StreamEventsOptions = {}): AsyncIterable<RunEvent> {
    if (options.signal?.aborted) return;
    yield* this.#streamEventsPolling(runId, { ...options, seenIds: new Set<string>() });
  }

  /** Short alias for `streamEvents`. */
  stream(runId: string, options?: StreamEventsOptions): AsyncIterable<RunEvent> {
    return this.streamEvents(runId, options);
  }

  /**
   * Stream the unified {@link AntpathEvent} envelope live over the coordinator
   * WebSocket. The Worker's ticket broker authorizes the connection (workspace
   * token → short-lived coordinator ticket); the shared client replays from
   * the cursor, tails live, and resumes exactly-once across reconnects. The
   * ticket is re-minted on each (re)connect so a long run never outlives it.
   */
  async *streamEnvelopes(runId: string, options: StreamEnvelopesOptions = {}): AsyncIterable<AntpathEvent> {
    const first = await operations.getCoordinatorTicket(this.#http, runId);
    yield* streamCoordinatorEvents({
      wsUrl: first.wsUrl,
      from: options.from ?? 0,
      fetchTicket: async () => (await operations.getCoordinatorTicket(this.#http, runId)).ticket,
      ...(options.signal ? { signal: options.signal } : {})
    });
  }

  async *#streamEventsPolling(
    runId: string,
    options: StreamEventsOptions & { seenIds: Set<string> }
  ): AsyncIterable<RunEvent> {
    const intervalMs = options.intervalMs ?? 1_000;
    const signal = options.signal;
    while (!signal?.aborted) {
      const events = await this.listEvents(runId);
      for (const event of events) {
        if (!options.seenIds.has(event.id)) {
          options.seenIds.add(event.id);
          yield event;
        }
      }
      const run = await this.getRun(runId);
      if (isTerminal(run.status)) return;
      // `sleep` rejects on abort — treat that as a graceful stop.
      try {
        await sleep(intervalMs, signal);
      } catch {
        return;
      }
    }
  }

  /**
   * Poll the run record until it reaches a terminal status (succeeded,
   * failed, terminated). Throws if `timeoutMs` elapses first.
   */
  async waitForRun(runId: string, options: WaitForRunOptions = {}): Promise<Run> {
    const intervalMs = options.intervalMs ?? 1_500;
    const timeoutMs = options.timeoutMs;
    const signal = options.signal;
    const deadline = typeof timeoutMs === "number" ? Date.now() + timeoutMs : Number.POSITIVE_INFINITY;
    while (!signal?.aborted) {
      const run = await this.getRun(runId);
      if (isTerminal(run.status)) return run;
      if (Date.now() >= deadline) {
        throw new Error(`AntpathClient.waitForRun: timeout after ${timeoutMs}ms`);
      }
      await sleep(intervalMs, signal);
    }
    throw new Error("AntpathClient.waitForRun: aborted");
  }

  /** Short alias for `waitForRun`. */
  wait(runId: string, options?: WaitForRunOptions): Promise<Run> {
    return this.waitForRun(runId, options);
  }

  listOutputs(runId: string): Promise<readonly Output[]> {
    return operations.listOutputs(this.#http, runId);
  }

  /** Short alias for `listOutputs`. */
  outputs(runId: string): Promise<readonly Output[]> {
    return this.listOutputs(runId);
  }

  createOutputLink(runId: string, outputId: string): Promise<SignedOutputLink> {
    return operations.createOutputLink(this.#http, runId, outputId);
  }

  /**
   * Download captured deliverables. Omit `selector` to receive the full
   * outputs namespace as a zip; pass an id, Output object, or path selector
   * to receive one file's raw bytes.
   */
  downloadOutput(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array>;
  downloadOutput(runId: string, selector: undefined, options?: OutputDownloadOptions): Promise<Uint8Array>;
  downloadOutput(runId: string, selector: OutputFileSelector, options?: OutputDownloadOptions): Promise<Uint8Array>;
  async downloadOutput(
    runId: string,
    selectorOrOptions?: OutputFileSelector | OutputDownloadOptions,
    options?: OutputDownloadOptions
  ): Promise<Uint8Array> {
    const hasSelector = selectorOrOptions !== undefined && !isOutputDownloadOptionsOnly(selectorOrOptions);
    const selector = hasSelector ? selectorOrOptions as OutputFileSelector : undefined;
    const to = hasSelector ? options?.to : (selectorOrOptions as OutputDownloadOptions | undefined)?.to ?? options?.to;
    let bytes: Uint8Array;
    if (selector === undefined) {
      bytes = await operations.downloadOutputs(this.#http, runId);
    } else {
      const output = isOutputPathSelector(selector)
        ? resolveOutputFileSelector(await operations.listOutputs(this.#http, runId), selector, runId)
        : resolveOutputFileSelector([], selector, runId);
      const { response } = await this.#http.download(
        `/api/runs/${encodeURIComponent(runId)}/outputs/${encodeURIComponent(output.id)}/download`
      );
      bytes = new Uint8Array(await response.arrayBuffer());
    }
    return writeOptionalFile(bytes, to);
  }

  /**
   * Bundle the per-run debug artifacts antpath captures automatically:
   *
   *   - `runtime/{stdout,stderr,args}.log` — runtime process diagnostics.
   *   - `host/...` — managed host logs when the platform includes them.
   * These all live in the run's `logs` namespace (`runs/<id>/logs/`).
   * Each is downloaded through the gated `/logs/:id/download` endpoint,
   * decoded as UTF-8 text when the content type looks textual, and
   * surfaced as raw bytes (base64) otherwise. The call is best-effort: a
   * download failure for one file does not block the others; the failing
   * entry lands in `errors` with the underlying message.
   *
   * Use this when a run failed or behaved oddly and you want all the
   * post-mortem material in one round-trip — no need to wire
   * `listOutputs` + `createOutputLink` by hand.
   */
  async getRunDebugLogs(runId: string): Promise<RunDebugLogs> {
    // The `logs` namespace IS the diagnostics surface — everything it
    // lists is a debug artifact, so no client-side prefix filter.
    const matches = await operations.listLogs(this.#http, runId);
    const logs: RunDebugLog[] = [];
    const errors: RunDebugLogError[] = [];
    for (const out of matches) {
      const filename = out.filename ?? "(unnamed)";
      try {
        const { response } = await this.#http.download(
          `/api/runs/${runId}/logs/${out.id}/download`
        );
        const buf = await response.arrayBuffer();
        const bytes = new Uint8Array(buf);
        const contentType = out.contentType ?? "application/octet-stream";
        const isText = /^(text\/|application\/json)/.test(contentType);
        const bytesBase64 = bytesToBase64(bytes);
        logs.push({
          filename,
          sizeBytes: out.sizeBytes ?? bytes.byteLength,
          contentType,
          createdAt: out.createdAt ?? new Date(0).toISOString(),
          ...(isText ? { text: new TextDecoder().decode(bytes) } : {}),
          bytesBase64
        });
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        errors.push({ filename, message });
      }
    }
    return { runId, logs, errors };
  }

  /** Short alias for `getRunDebugLogs`. */
  debugLogs(runId: string): Promise<RunDebugLogs> {
    return this.getRunDebugLogs(runId);
  }

  cancelRun(runId: string): Promise<void> {
    return operations.cancelRun(this.#http, runId);
  }

  /** Short alias for `cancelRun`. */
  cancel(runId: string): Promise<void> {
    return this.cancelRun(runId);
  }

  deleteRun(runId: string): Promise<void> {
    return operations.deleteRun(this.#http, runId);
  }

  /** Short alias for `deleteRun`. */
  delete(runId: string): Promise<void> {
    return this.deleteRun(runId);
  }

  /**
   * Delete a workspace asset blob from the shared content-addressed store
   * (`assets/<workspaceId>/<hash>`). Accepts `sha256:<hex>` or a bare
   * 64-hex digest. Runs that already snapshotted the asset are unaffected.
   */
  deleteWorkspaceAsset(hash: string): Promise<void> {
    return operations.deleteWorkspaceAsset(this.#http, hash);
  }

  whoami(): Promise<WhoAmI> {
    return operations.whoami(this.#http);
  }

  /**
   * Download EVERYTHING about a run as one zip, assembled client-side
   * from the public read endpoints (`getRun` + `listEvents` +
   * `listOutputs` + per-output `/download`). Organised into the four
   * namespace folders: `metadata/`, `events/`, `outputs/` (deliverables),
   * `logs/` (`runtime/`, `host/`, `provider-proxy/`, `control-plane/`
   * diagnostics), plus a `manifest.json`. Pass `to` to also write the
   * bytes to a file path while still returning the bytes.
   */
  async download(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.download(this.#http, runId), options?.to);
  }

  /** Download only the run's deliverables (the `outputs` namespace) as a zip. */
  async downloadOutputs(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadOutputs(this.#http, runId), options?.to);
  }

  /** Download only the platform diagnostics (the `logs` namespace) as a zip. */
  async downloadLogs(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadLogs(this.#http, runId), options?.to);
  }

  /** Download only the indexed event archive (the `events` namespace) as a zip. */
  async downloadEvents(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadEvents(this.#http, runId), options?.to);
  }

  /** Download only the run record (the `metadata` namespace) as a zip. */
  async downloadMetadata(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadMetadata(this.#http, runId), options?.to);
  }
}

// `Run.status` is a loose `string` on the wire shape, so we membership-test
// against the canonical terminal set rather than re-deriving one (which is how
// `timed_out` got dropped from the old hardcoded list).
const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

function isTerminal(status: string | undefined): boolean {
  return typeof status === "string" && TERMINAL_STATUSES.has(status);
}

function isOutputPathSelector(selector: OutputFileSelector): selector is OutputFilePathSelector {
  return Boolean(selector && typeof selector === "object" && "path" in selector);
}

function isOutputDownloadOptionsOnly(
  value: OutputFileSelector | OutputDownloadOptions
): value is OutputDownloadOptions {
  return Boolean(
    value &&
      typeof value === "object" &&
      "to" in value &&
      !("id" in value) &&
      !("path" in value)
  );
}

function resolveOutputFileSelector(
  outputs: readonly Output[],
  selector: OutputFileSelector,
  runId: string
): Output {
  if (isOutputPathSelector(selector)) {
    const target = normalizeOutputLookupPath(selector.path);
    if (!target) {
      throw new RunStateError("AntpathClient.downloadOutput: output path must be non-empty", {
        runId,
        path: selector.path
      });
    }
    const matches = outputs.filter((output) => {
      if (typeof output.filename !== "string") return false;
      const filename = normalizeOutputLookupPath(output.filename);
      if (selector.match === "suffix") {
        return filename === target || filename.endsWith(`/${target}`);
      }
      return filename === target;
    });
    if (matches.length === 1) return matches[0]!;
    if (matches.length > 1) {
      throw new RunStateError(
        `AntpathClient.downloadOutput: output path "${selector.path}" matched multiple files`,
        { runId, path: selector.path, matches: matches.map((output) => output.filename ?? output.id) }
      );
    }
    throw new RunStateError(`AntpathClient.downloadOutput: output path "${selector.path}" was not found`, {
      runId,
      path: selector.path
    });
  }
  if (typeof selector.id !== "string" || selector.id.length === 0) {
    throw new RunStateError("AntpathClient.downloadOutput: selector must include an output id or path", { runId });
  }
  return { ...selector, id: selector.id };
}

function normalizeOutputLookupPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\/+/, "");
}

async function writeOptionalFile(bytes: Uint8Array, to?: string): Promise<Uint8Array> {
  if (to !== undefined) {
    const { writeFile } = await import("node:fs/promises");
    await writeFile(to, bytes);
  }
  return bytes;
}

function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error("aborted"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(new Error("aborted"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/**
 * Encode a byte array as base64. Uses Node's `Buffer` when available
 * (the SDK ships as a Node tarball; this is the hot path), and falls
 * back to `btoa` for browser/Workers runtimes that pull the SDK in
 * without Buffer.
 */
function bytesToBase64(bytes: Uint8Array): string {
  const BufferCtor = (globalThis as { Buffer?: { from: (b: Uint8Array) => { toString: (enc: string) => string } } }).Buffer;
  if (BufferCtor) {
    return BufferCtor.from(bytes).toString("base64");
  }
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]!);
  }
  return globalThis.btoa(binary);
}

function generateIdempotencyKey(): string {
  const cryptoObj = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (cryptoObj?.randomUUID) return cryptoObj.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function normalisePrompt(input: string | readonly string[]): readonly string[] {
  if (typeof input === "string") {
    if (!input) {
      throw new Error("AntpathClient.submitRun: prompt must be a non-empty string");
    }
    return [input];
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new Error("AntpathClient.submitRun: prompt must be a non-empty string or string array");
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new Error("AntpathClient.submitRun: prompt segments must be non-empty strings");
    }
  }
  return [...input];
}

/**
 * Walk the user-provided `Skill[]`, validating each instance and
 * producing:
 *   - `skillRefs[]` — the wire entries for `submission.skills[]`, with
 *     inline refs assigned positional slot ids (`transient-0`, …).
 *   - `inlineBundles[]` — the bytes for each inline skill,
 *     parallel-indexed by slot.
 *
 * Throws on consumed Skills (the user reused a draft after a prior
 * `submitRun` call) so that mistake is loud, not silent.
 */
/**
 * Walk the user-provided Skill[], materialize every draft to assets, and return
 * the wire-shape refs.
 */
async function materializeSkills(
  http: import("./asset-upload.js").AssetsHttpClient,
  skills: readonly Skill[]
): Promise<readonly SkillRef[]> {
  const out: SkillRef[] = [];
  for (let i = 0; i < skills.length; i++) {
    const entry = skills[i];
    if (!(entry instanceof Skill)) {
      throw new Error(`AntpathClient.submitRun: skills[${i}] must be a Skill instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AntpathClient.submitRun: skills[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AntpathClient.submitRun: skills[${i}] is draft but has no bytes`);
      }
      const uploaded = await uploadAsset({
        http,
        bytes: bundle.bytes,
        hash: bundle.contentHash
      });
      out.push({
        kind: "asset",
        assetId: uploaded.assetId,
        name: bundle.name
      });
      continue;
    }
    // Already-materialized asset ref.
    out.push(ref);
  }
  return out;
}

/** Materialize draft AgentsMd[] to assets; pass-through any already-materialized refs. */
async function materializeAgentsMd(
  http: import("./asset-upload.js").AssetsHttpClient,
  agentsMds: readonly AgentsMd[]
): Promise<readonly AgentsMdRef[]> {
  const out: AgentsMdRef[] = [];
  for (let i = 0; i < agentsMds.length; i++) {
    const entry = agentsMds[i];
    if (!(entry instanceof AgentsMd)) {
      throw new Error(`AntpathClient.submitRun: agentsMd[${i}] must be an AgentsMd instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AntpathClient.submitRun: agentsMd[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AntpathClient.submitRun: agentsMd[${i}] is draft but has no bytes`);
      }
      const uploaded = await uploadAsset({ http, bytes: bundle.bytes, hash: bundle.contentHash });
      out.push({
        kind: "asset",
        assetId: uploaded.assetId,
        name: bundle.name
      });
      continue;
    }
    out.push(ref);
  }
  return out;
}

/** Materialize draft File[] to assets; pass-through any already-materialized refs. */
async function materializeFiles(
  http: import("./asset-upload.js").AssetsHttpClient,
  files: readonly File[]
): Promise<readonly FileRef[]> {
  const out: FileRef[] = [];
  for (let i = 0; i < files.length; i++) {
    const entry = files[i];
    if (!(entry instanceof File)) {
      throw new Error(`AntpathClient.submitRun: files[${i}] must be a File instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AntpathClient.submitRun: files[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AntpathClient.submitRun: files[${i}] is draft but has no bytes`);
      }
      const uploaded = await uploadAsset({ http, bytes: bundle.bytes, hash: bundle.contentHash });
      out.push(
        bundle.mountPath !== undefined
          ? {
              kind: "asset",
              assetId: uploaded.assetId,
              name: bundle.name,
              mountPath: bundle.mountPath
            }
          : {
              kind: "asset",
              assetId: uploaded.assetId,
              name: bundle.name
            }
      );
      continue;
    }
    out.push(ref);
  }
  return out;
}


function mergeMcpServers(
  inputs: readonly McpServer[],
  explicitSecrets: readonly PlatformMcpServerSecret[]
): {
  submissionMcpServers: ReadonlyArray<McpServerRef | { readonly kind: "workspace"; readonly id: string }>;
  mergedMcpSecrets: readonly PlatformMcpServerSecret[];
} {
  const submissionMcpServers: Array<McpServerRef | { readonly kind: "workspace"; readonly id: string }> = [];
  const secretByName = new Map<string, PlatformMcpServerSecret>();
  for (const secret of explicitSecrets) {
    secretByName.set(secret.name, secret);
  }
  for (let i = 0; i < inputs.length; i++) {
    const entry = inputs[i];
    if (!(entry instanceof McpServer)) {
      throw new Error(`AntpathClient.submitRun: mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw new Error(
          `AntpathClient.submitRun: mcpServers[${i}].url conflicts with secrets.mcpServers["${secret.name}"]`
        );
      }
      secretByName.set(secret.name, secret);
    }
  }
  return {
    submissionMcpServers,
    mergedMcpSecrets: Array.from(secretByName.values())
  };
}

/**
 * Merge `ProxyEndpoint`-derived auth entries with any
 * `secrets.proxyEndpointAuth` the caller passed explicitly. Per-instance
 * auth values win on the same `name`; a type mismatch (e.g. instance
 * declares `bearer` but secrets carry `header` for the same name) is a
 * call-site error and we throw at the SDK boundary instead of letting
 * the BFF reject the submission an HTTP request later.
 */
function mergeProxyEndpointAuth(
  fromInstances: readonly PlatformProxyEndpointAuth[],
  fromExplicitSecrets: readonly PlatformProxyEndpointAuth[]
): readonly PlatformProxyEndpointAuth[] {
  if (fromInstances.length === 0 && fromExplicitSecrets.length === 0) return [];
  const byName = new Map<string, PlatformProxyEndpointAuth>();
  for (const entry of fromExplicitSecrets) {
    byName.set(entry.name, entry);
  }
  for (const entry of fromInstances) {
    const existing = byName.get(entry.name);
    if (existing && existing.value.type !== entry.value.type) {
      throw new Error(
        `AntpathClient.submitRun: proxyEndpoint "${entry.name}" auth type conflicts ` +
          `with secrets.proxyEndpointAuth (instance=${entry.value.type}, secrets=${existing.value.type})`
      );
    }
    byName.set(entry.name, entry);
  }
  return Array.from(byName.values());
}

// Side-channel re-exports keep the proxy wire types reachable from
// `import type { … } from "antpath/client"` without forcing consumers
// to learn an additional entry point.
export type { PlatformProxyEndpoint, PlatformProxyEndpointAuth };
