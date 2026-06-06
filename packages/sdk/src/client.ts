import {
  AexError,
  DEFAULT_CREDENTIAL_MODE,
  DEFAULT_RUN_PROVIDER,
  HttpClient,
  RUNTIME_KINDS,
  RunStateError,
  operations,
  parseCredentialMode,
  streamCoordinatorEvents,
  type AexEvent,
  type AgentsMdRecord,
  type CredentialMode,
  type AgentsMdRef,
  type DebugSink,
  type FetchLike,
  type FileRecord,
  type FileRef,
  type McpServerRef,
  type Output,
  type OutputMode,
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
} from "@aexhq/contracts";
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { AgentsMd } from "./agents-md.js";
import { File } from "./file.js";
import { McpServer } from "./mcp-server.js";
import { ProxyEndpoint, splitProxyEndpoints } from "./proxy-endpoint.js";
import { Skill } from "./skill.js";

export interface AgentExecutorOptions {
  /** Workspace-scoped SDK API token. */
  readonly apiToken: string;
  /**
   * API plane root, e.g. `https://aex.example.com`. Optional —
   * defaults to the canonical hosted URL (`https://api.aex.dev`).
   * Override for local, staging, or other hosted aex API planes.
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
 *   - `secrets.apiKey` — REQUIRED: the provider key for the selected
 *     `provider`. The platform never holds a long-lived provider key on
 *     your behalf.
 *
 * `idempotencyKey` is auto-generated when omitted; pass one explicitly
 * if you want client-driven retry safety across process restarts.
 */
export interface SubmitRunOptions {
  /**
   * Credential source for upstream provider access. Omitted defaults to
   * `"byok"`, which requires `secrets.apiKey`.
   * `"managed"` is reserved for paid managed-key mode and currently fails
   * closed until the hosted private implementation is wired.
   */
  readonly credentialMode?: CredentialMode;
  /**
   * Provider selector. Optional — defaults to
   * {@link DEFAULT_RUN_PROVIDER} (`"anthropic"`). Selects which upstream
   * model route the managed provider-proxy uses; the BYOK key for it is
   * supplied as `secrets.apiKey`.
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
   * Output capture policy for the run's output files.
   *
   * - `allowedDirs` omitted: every regular file the session creates or
   *   modifies is captured.
   * - `allowedDirs` present: the listed roots narrow capture to those paths.
   * - `deniedDirs` subtracts noise from the allowed roots.
   *
   * Captured bytes land in private storage and can be retrieved via
   * `client.outputs(runId)` / `client.download(runId)`. See
   * `packages/sdk/docs/outputs.md` for the full contract.
   */
  readonly outputs?: {
    readonly allowedDirs?: readonly string[];
    readonly deniedDirs?: readonly string[];
  };
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
  /**
   * Assistant-output granularity. `"buffered"` (default) delivers one event per
   * assistant message; `"stream"` delivers per-token text deltas for live
   * typing UIs.
   */
  readonly outputMode?: OutputMode;
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
 * Unified user-facing client for the aex platform. The same class
 * powers the published `@aexhq/sdk` SDK and (under the hood) every host-side
 * subcommand of the in-container `aex` CLI. All operations talk to
 * the dashboard BFF and operate on durable run records.
 *
 * The SDK never asks the caller for a workspace id — workspace identity
 * is derived server-side from the API token on every request. Use
 * `client.whoami()` if you want to introspect which workspace the
 * token resolves to.
 */
export class AgentExecutor {
  readonly #http: HttpClient;
  /** The same fetch the HttpClient uses, kept for direct bootstrap uploads. */
  readonly #fetch: FetchLike | undefined;
  readonly skills: SkillsClient;
  readonly agentsMd: AgentsMdClient;
  readonly files: FilesClient;

  constructor(options: AgentExecutorOptions) {
    if (!options.apiToken) {
      throw new Error("AgentExecutor: apiToken is required");
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
    this.#fetch = options.fetch;
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
   * ids. The bytes persist on aex; the user can browse and
   * download the resulting workspace skill from the dashboard.
   */
  async submitRun(options: SubmitRunOptions): Promise<string> {
    if (!options || typeof options !== "object") {
      throw new Error("AgentExecutor.submitRun: options is required");
    }
    const provider: RunProvider = options.provider ?? DEFAULT_RUN_PROVIDER;
    const credentialMode = parseCredentialMode(options.credentialMode);
    if (credentialMode === "managed") {
      throw new AexError(
        "CREDENTIAL_INVALID",
        "AgentExecutor.submitRun: credentialMode \"managed\" is not available without a private managed-key implementation"
      );
    }
    if (!options.secrets) {
      throw new Error("AgentExecutor.submitRun: secrets is required");
    }
    // The BYOK provider key (for the selected `provider`) is required. The
    // shared parser re-runs this check on the server; failing early here
    // gives the caller a synchronous error before any network call.
    if (typeof options.secrets.apiKey !== "string" || !options.secrets.apiKey) {
      throw new Error("AgentExecutor.submitRun: secrets.apiKey is required");
    }
    if (typeof options.model !== "string" || !options.model) {
      throw new Error("AgentExecutor.submitRun: model is required");
    }
    const prompt = normalisePrompt(options.prompt);
    const { endpoints: proxyEndpointDeclarations, auth: proxyEndpointAuthFromInstances } =
      splitProxyEndpoints(options.proxyEndpoints ?? []);
    const mergedProxyAuth = mergeProxyEndpointAuth(
      proxyEndpointAuthFromInstances,
      options.secrets.proxyEndpointAuth ?? []
    );

    // Walk Skill / AgentsMd / File instances. Drafts are declared as direct
    // inputs on the submit request, then uploaded to the run bootstrap target
    // after the control plane accepts the run. Already-materialized asset refs
    // still pass through unchanged.
    const preparedSkills = prepareSkills(options.skills ?? []);
    const preparedAgentsMd = prepareAgentsMd(options.agentsMd ?? []);
    const preparedFiles = prepareFiles(options.files ?? []);
    const directInputs = [
      ...preparedSkills.directInputs,
      ...preparedAgentsMd.directInputs,
      ...preparedFiles.directInputs
    ];
    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      options.secrets.mcpServers ?? []
    );

    const submission: PlatformSubmission = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      prompt,
      skills: preparedSkills.refs,
      agentsMd: preparedAgentsMd.refs,
      files: preparedFiles.refs,
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
      ...(options.outputs &&
      ((options.outputs.allowedDirs?.length ?? 0) > 0 || (options.outputs.deniedDirs?.length ?? 0) > 0)
        ? { outputs: options.outputs }
        : {}),
      // Pass-through `builtins` verbatim — including an empty array,
      // which is the "disable all builtins" signal. Distinguish from
      // omitted (default applies) via `!== undefined`.
      ...(options.builtins !== undefined ? { builtins: options.builtins } : {}),
      ...(options.outputMode !== undefined ? { outputMode: options.outputMode } : {})
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
      throw new AexError(
        "RUNTIME_UNSUPPORTED",
        `AgentExecutor.submitRun: runtime must be one of: ${RUNTIME_KINDS.join(", ")} ` +
          `(got ${JSON.stringify(options.runtime)})`
      );
    }

    const submitRequest =
      directInputs.length > 0
        ? {
            ...request,
            bootstrapMode: "direct",
            directInputs: directInputs.map(({ bytes: _bytes, ...descriptor }) => descriptor)
          }
        : request;

    const run = await operations.submitRun(
      this.#http,
      submitRequest as PlatformRunSubmissionInput
    ) as Run & DirectBootstrapSubmitResponse;
    const runId = getSubmittedRunId(run);
    if (directInputs.length > 0) {
      await completeDirectBootstrap({
        response: run,
        directInputs,
        hasRunAcceptedBootstrap: async () => hasRunAcceptedBootstrap(await operations.getRun(this.#http, runId)),
        ...(options.signal ? { signal: options.signal } : {}),
        ...(this.#fetch ? { fetch: this.#fetch } : {})
      });
    }
    return runId;
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
   * Stream the unified {@link AexEvent} envelope live over the coordinator
   * WebSocket. The Worker's ticket broker authorizes the connection (workspace
   * token → short-lived coordinator ticket); the shared client replays from
   * the cursor, tails live, and resumes exactly-once across reconnects. The
   * ticket is re-minted on each (re)connect so a long run never outlives it.
   */
  async *streamEnvelopes(runId: string, options: StreamEnvelopesOptions = {}): AsyncIterable<AexEvent> {
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
        throw new Error(`AgentExecutor.waitForRun: timeout after ${timeoutMs}ms`);
      }
      await sleep(intervalMs, signal);
    }
    throw new Error("AgentExecutor.waitForRun: aborted");
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
   * Download EVERYTHING public about a run as one zip, assembled client-side
   * from the public read endpoints (`getRun` + `listEvents` +
   * `listOutputs` + per-output `/download`). Organised into the namespace
   * folders `metadata/`, `events/`, and `outputs/`, plus a `manifest.json`.
   * Pass `to` to also write the
   * bytes to a file path while still returning the bytes.
   */
  async download(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.download(this.#http, runId), options?.to);
  }

  /** Download only the run's deliverables (the `outputs` namespace) as a zip. */
  async downloadOutputs(runId: string, options?: OutputDownloadOptions): Promise<Uint8Array> {
    return writeOptionalFile(await operations.downloadOutputs(this.#http, runId), options?.to);
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
const DIRECT_BOOTSTRAP_RETRY_STATUSES = new Set([408, 425, 429, 502, 503, 504]);
const DIRECT_BOOTSTRAP_INITIAL_BACKOFF_MS = 100;
const DIRECT_BOOTSTRAP_MAX_BACKOFF_MS = 1_000;
const DIRECT_BOOTSTRAP_ATTEMPT_TIMEOUT_MS = 10_000;

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
      throw new RunStateError("AgentExecutor.downloadOutput: output path must be non-empty", {
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
        `AgentExecutor.downloadOutput: output path "${selector.path}" matched multiple files`,
        { runId, path: selector.path, matches: matches.map((output) => output.filename ?? output.id) }
      );
    }
    throw new RunStateError(`AgentExecutor.downloadOutput: output path "${selector.path}" was not found`, {
      runId,
      path: selector.path
    });
  }
  if (typeof selector.id !== "string" || selector.id.length === 0) {
    throw new RunStateError("AgentExecutor.downloadOutput: selector must include an output id or path", { runId });
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

function generateIdempotencyKey(): string {
  const cryptoObj = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (cryptoObj?.randomUUID) return cryptoObj.randomUUID();
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function normalisePrompt(input: string | readonly string[]): readonly string[] {
  if (typeof input === "string") {
    if (!input) {
      throw new Error("AgentExecutor.submitRun: prompt must be a non-empty string");
    }
    return [input];
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new Error("AgentExecutor.submitRun: prompt must be a non-empty string or string array");
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new Error("AgentExecutor.submitRun: prompt segments must be non-empty strings");
    }
  }
  return [...input];
}

type DirectInputRole = "skill" | "agentsMd" | "file";

interface DirectInputUpload {
  readonly inputId: string;
  readonly role: DirectInputRole;
  readonly assetId: string;
  readonly name: string;
  readonly sha256: string;
  readonly sizeBytes: number;
  readonly contentType: string;
  readonly mountPath?: string;
  readonly bytes: Uint8Array;
}

interface PreparedDirectRefs<T> {
  readonly refs: readonly T[];
  readonly directInputs: readonly DirectInputUpload[];
}

interface DirectBootstrapSubmitResponse {
  readonly id?: string;
  readonly runId?: string;
  readonly bootstrapStatusUrl?: string;
  readonly bootstrapToken?: string;
  readonly bootstrapExpiresAt?: string;
  readonly uploadBaseUrl?: string;
  readonly routingHeaders?: Record<string, string>;
}

interface DirectBootstrapReady {
  readonly uploadBaseUrl: string;
  readonly routingHeaders?: Record<string, string>;
  readonly abortUrl?: string;
  readonly commitUrl?: string;
}

/** Walk Skill[] and turn drafts into direct-bootstrap descriptors. */
function prepareSkills(skills: readonly Skill[]): PreparedDirectRefs<SkillRef> {
  const refs: SkillRef[] = [];
  const directInputs: DirectInputUpload[] = [];
  for (let i = 0; i < skills.length; i++) {
    const entry = skills[i];
    if (!(entry instanceof Skill)) {
      throw new Error(`AgentExecutor.submitRun: skills[${i}] must be a Skill instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AgentExecutor.submitRun: skills[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AgentExecutor.submitRun: skills[${i}] is draft but has no bytes`);
      }
      const input = directInputFor({
        role: "skill",
        index: i,
        name: bundle.name,
        contentHash: bundle.contentHash,
        bytes: bundle.bytes,
        contentType: "application/zip"
      });
      directInputs.push(input);
      refs.push({
        kind: "asset",
        assetId: input.assetId,
        name: bundle.name
      });
      continue;
    }
    // Already-materialized asset ref.
    refs.push(ref);
  }
  return { refs, directInputs };
}

/** Walk AgentsMd[] and turn drafts into direct-bootstrap descriptors. */
function prepareAgentsMd(agentsMds: readonly AgentsMd[]): PreparedDirectRefs<AgentsMdRef> {
  const refs: AgentsMdRef[] = [];
  const directInputs: DirectInputUpload[] = [];
  for (let i = 0; i < agentsMds.length; i++) {
    const entry = agentsMds[i];
    if (!(entry instanceof AgentsMd)) {
      throw new Error(`AgentExecutor.submitRun: agentsMd[${i}] must be an AgentsMd instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AgentExecutor.submitRun: agentsMd[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AgentExecutor.submitRun: agentsMd[${i}] is draft but has no bytes`);
      }
      const input = directInputFor({
        role: "agentsMd",
        index: i,
        name: bundle.name,
        contentHash: bundle.contentHash,
        bytes: bundle.bytes,
        contentType: "application/zip"
      });
      directInputs.push(input);
      refs.push({
        kind: "asset",
        assetId: input.assetId,
        name: bundle.name
      });
      continue;
    }
    refs.push(ref);
  }
  return { refs, directInputs };
}

/** Walk File[] and turn drafts into direct-bootstrap descriptors. */
function prepareFiles(files: readonly File[]): PreparedDirectRefs<FileRef> {
  const refs: FileRef[] = [];
  const directInputs: DirectInputUpload[] = [];
  for (let i = 0; i < files.length; i++) {
    const entry = files[i];
    if (!(entry instanceof File)) {
      throw new Error(`AgentExecutor.submitRun: files[${i}] must be a File instance`);
    }
    if (entry.isConsumed) {
      throw new Error(`AgentExecutor.submitRun: files[${i}] was already consumed by a prior submitRun`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new Error(`AgentExecutor.submitRun: files[${i}] is draft but has no bytes`);
      }
      const input = directInputFor({
        role: "file",
        index: i,
        name: bundle.name,
        contentHash: bundle.contentHash,
        bytes: bundle.bytes,
        contentType: "application/zip",
        ...(bundle.mountPath ? { mountPath: bundle.mountPath } : {})
      });
      directInputs.push(input);
      refs.push(
        bundle.mountPath !== undefined
          ? {
              kind: "asset",
              assetId: input.assetId,
              name: bundle.name,
              mountPath: bundle.mountPath
            }
          : {
              kind: "asset",
              assetId: input.assetId,
              name: bundle.name
            }
      );
      continue;
    }
    refs.push(ref);
  }
  return { refs, directInputs };
}

function directInputFor(args: {
  readonly role: DirectInputRole;
  readonly index: number;
  readonly name: string;
  readonly contentHash: string;
  readonly bytes: Uint8Array;
  readonly contentType: string;
  readonly mountPath?: string;
}): DirectInputUpload {
  const sha256 = args.contentHash.startsWith("sha256:")
    ? args.contentHash
    : `sha256:${args.contentHash}`;
  const hashHex = sha256.slice("sha256:".length);
  if (!/^[0-9a-f]{64}$/.test(hashHex)) {
    throw new Error(`AgentExecutor.submitRun: ${args.role}[${args.index}] content hash must be sha256:<64-hex>`);
  }
  return {
    inputId: `input_${args.role}_${args.index}_${hashHex}`,
    role: args.role,
    assetId: `asset_${hashHex}`,
    name: args.name,
    sha256,
    sizeBytes: args.bytes.byteLength,
    contentType: args.contentType,
    ...(args.mountPath ? { mountPath: args.mountPath } : {}),
    bytes: args.bytes
  };
}

function getSubmittedRunId(response: Run & DirectBootstrapSubmitResponse): string {
  const id = response.id ?? response.runId;
  if (typeof id !== "string" || id.length === 0) {
    throw new Error("AgentExecutor.submitRun: submit response did not include a run id");
  }
  return id;
}

async function completeDirectBootstrap(args: {
  readonly response: DirectBootstrapSubmitResponse;
  readonly directInputs: readonly DirectInputUpload[];
  readonly fetch?: FetchLike;
  readonly hasRunAcceptedBootstrap?: () => Promise<boolean>;
  readonly signal?: AbortSignal;
}): Promise<void> {
  const token = args.response.bootstrapToken;
  const statusUrl = args.response.bootstrapStatusUrl;
  if (typeof token !== "string" || token.length === 0 || typeof statusUrl !== "string" || statusUrl.length === 0) {
    return;
  }
  const fetchImpl = args.fetch ?? globalThis.fetch.bind(globalThis);
  const useNodeTransport = args.fetch === undefined;
  const deadline = bootstrapDeadline(args.response.bootstrapExpiresAt);
  let target: DirectBootstrapReady | undefined;
  try {
    target = resolveBootstrapReady(args.response) ?? await pollBootstrapReady({
      fetchImpl,
      statusUrl,
      token,
      deadline,
      ...(args.signal ? { signal: args.signal } : {})
    });
    for (const input of args.directInputs) {
      await uploadDirectInput({
        fetchImpl,
        target,
        token,
        input,
        deadline,
        useNodeTransport,
        ...(args.signal ? { signal: args.signal } : {})
      });
    }
    await commitDirectInputs({
      fetchImpl,
      target,
      token,
      inputs: args.directInputs,
      deadline,
      useNodeTransport,
      ...(args.hasRunAcceptedBootstrap ? { hasRunAcceptedBootstrap: args.hasRunAcceptedBootstrap } : {}),
      ...(args.signal ? { signal: args.signal } : {})
    });
  } catch (err) {
    await abortDirectBootstrap({
      fetchImpl,
      token,
      statusUrl,
      ...(target ? { target } : {}),
      ...(args.signal ? { signal: args.signal } : {})
    }).catch(() => undefined);
    throw err;
  }
}

async function pollBootstrapReady(args: {
  readonly fetchImpl: FetchLike;
  readonly statusUrl: string;
  readonly token: string;
  readonly deadline: number;
  readonly signal?: AbortSignal;
}): Promise<DirectBootstrapReady> {
  while (!args.signal?.aborted) {
    if (Date.now() >= args.deadline) {
      throw new Error("AgentExecutor.submitRun: bootstrap target did not become ready before it expired");
    }
    const res = await args.fetchImpl(args.statusUrl, {
      method: "GET",
      headers: { authorization: `Bearer ${args.token}`, accept: "application/json" },
      ...(args.signal ? { signal: args.signal } : {})
    });
    if (res.ok) {
      const body = await res.json() as unknown;
      const ready = resolveBootstrapReady(body);
      if (ready) return ready;
    } else if (![202, 404, 425].includes(res.status)) {
      const detail = await res.text().catch(() => "");
      throw new Error(
        `AgentExecutor.submitRun: bootstrap status failed with ${res.status}` +
          (detail ? `: ${detail.slice(0, 300)}` : "")
      );
    }
    await sleep(250, args.signal);
  }
  throw new Error("AgentExecutor.submitRun: aborted");
}

function resolveBootstrapReady(value: unknown): DirectBootstrapReady | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as {
    readonly uploadBaseUrl?: unknown;
    readonly bootstrapUploadBaseUrl?: unknown;
    readonly routingHeaders?: unknown;
    readonly abortUrl?: unknown;
    readonly commitUrl?: unknown;
    readonly state?: unknown;
    readonly status?: unknown;
  };
  const base =
    typeof record.uploadBaseUrl === "string"
      ? record.uploadBaseUrl
      : typeof record.bootstrapUploadBaseUrl === "string"
        ? record.bootstrapUploadBaseUrl
        : undefined;
  if (!base) return undefined;
  const routingHeaders = isStringRecord(record.routingHeaders) ? record.routingHeaders : undefined;
  return {
    uploadBaseUrl: stripTrailingSlash(base),
    ...(routingHeaders ? { routingHeaders } : {}),
    ...(typeof record.abortUrl === "string" ? { abortUrl: record.abortUrl } : {}),
    ...(typeof record.commitUrl === "string" ? { commitUrl: record.commitUrl } : {})
  };
}

async function uploadDirectInput(args: {
  readonly fetchImpl: FetchLike;
  readonly target: DirectBootstrapReady;
  readonly token: string;
  readonly input: DirectInputUpload;
  readonly deadline: number;
  readonly useNodeTransport: boolean;
  readonly signal?: AbortSignal;
}): Promise<void> {
  let backoffMs = DIRECT_BOOTSTRAP_INITIAL_BACKOFF_MS;
  let lastError: Error | undefined;
  const uploadUrl = `${args.target.uploadBaseUrl}/inputs/${encodeURIComponent(args.input.inputId)}`;
  while (!args.signal?.aborted) {
    if (Date.now() >= args.deadline) {
      throw lastError ?? new Error("AgentExecutor.submitRun: bootstrap input upload did not complete before it expired");
    }
    let res: Response;
    try {
      res = await directBootstrapFetch({
        fetchImpl: args.fetchImpl,
        url: uploadUrl,
        deadline: args.deadline,
        operation: `input upload for ${args.input.inputId}`,
        useNodeTransport: args.useNodeTransport,
        ...(args.signal ? { signal: args.signal } : {}),
        init: {
          method: "PUT",
          headers: {
            authorization: `Bearer ${args.token}`,
            "content-type": args.input.contentType,
            "x-aex-input-sha256": args.input.sha256,
            "x-aex-input-size": String(args.input.sizeBytes),
            ...(args.target.routingHeaders ?? {})
          },
          body: args.input.bytes as unknown as BodyInit
        }
      });
    } catch (err) {
      if (args.signal?.aborted) throw err;
      lastError = bootstrapNetworkError(`input upload for ${args.input.inputId}`, err);
      backoffMs = await waitForBootstrapRetry(backoffMs, args.deadline, args.signal, lastError);
      continue;
    }
    if (res.ok) return;
    const detail = await res.text().catch(() => "");
    const err = new Error(
      `AgentExecutor.submitRun: bootstrap input upload failed for ${args.input.inputId} ` +
        `(status ${res.status})${detail ? `: ${detail.slice(0, 300)}` : ""}`
    );
    if (!DIRECT_BOOTSTRAP_RETRY_STATUSES.has(res.status)) {
      throw err;
    }
    lastError = err;
    backoffMs = await waitForBootstrapRetry(backoffMs, args.deadline, args.signal, lastError);
  }
  throw lastError ?? new Error("AgentExecutor.submitRun: aborted");
}

async function waitForBootstrapRetry(
  backoffMs: number,
  deadline: number,
  signal: AbortSignal | undefined,
  lastError: Error
): Promise<number> {
  const remainingMs = deadline - Date.now();
  if (remainingMs <= 0) throw lastError;
  await sleep(Math.min(backoffMs, remainingMs), signal);
  return Math.min(backoffMs * 2, DIRECT_BOOTSTRAP_MAX_BACKOFF_MS);
}

async function directBootstrapFetch(args: {
  readonly fetchImpl: FetchLike;
  readonly url: string;
  readonly init: RequestInit;
  readonly deadline: number;
  readonly operation: string;
  readonly useNodeTransport: boolean;
  readonly hasRunAcceptedBootstrap?: () => Promise<boolean>;
  readonly signal?: AbortSignal;
}): Promise<Response> {
  const remainingMs = args.deadline - Date.now();
  if (remainingMs <= 0) {
    throw new Error(`AgentExecutor.submitRun: bootstrap ${args.operation} did not complete before it expired`);
  }
  const timeoutMs = Math.min(DIRECT_BOOTSTRAP_ATTEMPT_TIMEOUT_MS, remainingMs);
  if (args.useNodeTransport) {
    return nodeDirectBootstrapFetch({
      url: args.url,
      init: args.init,
      timeoutMs,
      operation: args.operation,
      ...(args.hasRunAcceptedBootstrap ? { hasRunAcceptedBootstrap: args.hasRunAcceptedBootstrap } : {}),
      ...(args.signal ? { signal: args.signal } : {})
    });
  }
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  args.signal?.addEventListener("abort", onAbort, { once: true });
  const fetchPromise = args.fetchImpl(args.url, { ...args.init, signal: controller.signal });
  fetchPromise.catch(() => undefined);
  let timer: ReturnType<typeof setTimeout>;
  const timeoutPromise = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      reject(new Error(`AgentExecutor.submitRun: bootstrap ${args.operation} timed out after ${timeoutMs}ms`));
    }, timeoutMs);
  });
  try {
    return await Promise.race([fetchPromise, timeoutPromise]);
  } catch (err) {
    if (args.signal?.aborted) throw err;
    throw err;
  } finally {
    clearTimeout(timer!);
    args.signal?.removeEventListener("abort", onAbort);
  }
}

async function nodeDirectBootstrapFetch(args: {
  readonly url: string;
  readonly init: RequestInit;
  readonly timeoutMs: number;
  readonly operation: string;
  readonly hasRunAcceptedBootstrap?: () => Promise<boolean>;
  readonly signal?: AbortSignal;
}): Promise<Response> {
  const url = new URL(args.url);
  const requestImpl = url.protocol === "http:" ? httpRequest : url.protocol === "https:" ? httpsRequest : undefined;
  if (!requestImpl) {
    throw new Error(`AgentExecutor.submitRun: bootstrap ${args.operation} URL must use http or https`);
  }

  return await new Promise<Response>((resolve, reject) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let activeResponse: { destroy: (error?: Error) => void } | undefined;
    let acceptedPollStarted = false;
    const requestHeaders = normalizeBootstrapRequestHeaders(args.init.headers);
    if (!hasHeader(requestHeaders, "connection")) {
      requestHeaders.connection = "close";
    }

    const request = requestImpl(
      url,
      {
        method: args.init.method ?? "GET",
        headers: requestHeaders,
        agent: false
      },
      (res) => {
        activeResponse = res;
        const chunks: Uint8Array[] = [];
        res.on("data", (chunk: string | Uint8Array) => {
          chunks.push(typeof chunk === "string" ? new TextEncoder().encode(chunk) : new Uint8Array(chunk));
        });
        res.on("end", () => {
          if (settled) return;
          settled = true;
          cleanup();
          activeResponse = undefined;
          request.destroy();
          resolve(
            new Response(concatUint8Arrays(chunks), {
              status: res.statusCode ?? 599,
              statusText: res.statusMessage ?? "",
              headers: normalizeBootstrapResponseHeaders(res.headers)
            })
          );
        });
        res.on("error", fail);
      }
    );

    function cleanup(): void {
      if (timer) clearTimeout(timer);
      args.signal?.removeEventListener("abort", onAbort);
    }

    function fail(err: Error): void {
      if (settled) return;
      settled = true;
      cleanup();
      activeResponse?.destroy(err);
      request.destroy(err);
      reject(err);
    }

    function timeoutError(): Error {
      return new Error(`AgentExecutor.submitRun: bootstrap ${args.operation} timed out after ${args.timeoutMs}ms`);
    }

    function onAbort(): void {
      fail(new Error(`AgentExecutor.submitRun: bootstrap ${args.operation} aborted`));
    }

    function startAcceptedPoll(): void {
      if (!args.hasRunAcceptedBootstrap || acceptedPollStarted) return;
      acceptedPollStarted = true;
      void pollAcceptedBootstrapAfterRequestFinish({
        check: args.hasRunAcceptedBootstrap,
        isSettled: () => settled,
        ...(args.signal ? { signal: args.signal } : {})
      })
        .then((accepted) => {
          if (!accepted || settled) return;
          settled = true;
          cleanup();
          activeResponse?.destroy();
          request.destroy();
          resolve(
            new Response(JSON.stringify({ ok: true }), {
              status: 200,
              headers: { "content-type": "application/json" }
            })
          );
        })
        .catch(fail);
    }

    timer = setTimeout(() => fail(timeoutError()), args.timeoutMs);
    request.setTimeout(args.timeoutMs, () => fail(timeoutError()));
    request.on("error", fail);
    request.on("finish", startAcceptedPoll);
    args.signal?.addEventListener("abort", onAbort, { once: true });

    try {
      const body = normalizeBootstrapRequestBody(args.init.body);
      if (body !== undefined) request.write(body);
      request.end();
      startAcceptedPoll();
    } catch (err) {
      fail(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

async function pollAcceptedBootstrapAfterRequestFinish(args: {
  readonly check: () => Promise<boolean>;
  readonly isSettled: () => boolean;
  readonly signal?: AbortSignal;
}): Promise<boolean> {
  while (!args.isSettled()) {
    if (await checkRunAcceptedBootstrap(args.check)) return true;
    await sleep(250, args.signal);
  }
  return false;
}

function normalizeBootstrapRequestHeaders(headers: HeadersInit | undefined): Record<string, string> {
  const result: Record<string, string> = {};
  if (!headers) return result;
  if (headers instanceof Headers) {
    headers.forEach((value, key) => {
      result[key] = value;
    });
    return result;
  }
  if (Array.isArray(headers)) {
    for (const [key, value] of headers) result[key] = value;
    return result;
  }
  for (const [key, value] of Object.entries(headers)) {
    if (value !== undefined) result[key] = String(value);
  }
  return result;
}

function hasHeader(headers: Record<string, string>, name: string): boolean {
  const normalized = name.toLowerCase();
  return Object.keys(headers).some((key) => key.toLowerCase() === normalized);
}

function normalizeBootstrapResponseHeaders(headers: Record<string, string | string[] | undefined>): Headers {
  const result = new Headers();
  for (const [key, value] of Object.entries(headers)) {
    if (value === undefined) continue;
    if (Array.isArray(value)) {
      for (const item of value) result.append(key, item);
    } else {
      result.set(key, value);
    }
  }
  return result;
}

function normalizeBootstrapRequestBody(body: BodyInit | null | undefined): string | Uint8Array | undefined {
  if (body === null || body === undefined) return undefined;
  if (typeof body === "string") return body;
  if (body instanceof Uint8Array) return body;
  if (body instanceof ArrayBuffer) return new Uint8Array(body);
  if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
  if (body instanceof URLSearchParams) return body.toString();
  throw new Error("AgentExecutor.submitRun: unsupported bootstrap request body type");
}

function concatUint8Arrays(chunks: readonly Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0);
  const result = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

function bootstrapNetworkError(operation: string, value: unknown): Error {
  const message = value instanceof Error ? value.message : String(value);
  return new Error(`AgentExecutor.submitRun: bootstrap ${operation} failed: ${message}`);
}

async function commitDirectInputs(args: {
  readonly fetchImpl: FetchLike;
  readonly target: DirectBootstrapReady;
  readonly token: string;
  readonly inputs: readonly DirectInputUpload[];
  readonly deadline: number;
  readonly useNodeTransport: boolean;
  readonly hasRunAcceptedBootstrap?: () => Promise<boolean>;
  readonly signal?: AbortSignal;
}): Promise<void> {
  const commitUrl = args.target.commitUrl ?? `${args.target.uploadBaseUrl}/commit`;
  let backoffMs = DIRECT_BOOTSTRAP_INITIAL_BACKOFF_MS;
  let lastError: Error | undefined;
  while (!args.signal?.aborted) {
    if (Date.now() >= args.deadline) {
      throw lastError ?? new Error("AgentExecutor.submitRun: bootstrap commit did not complete before it expired");
    }
    let res: Response;
    try {
      res = await directBootstrapFetch({
        fetchImpl: args.fetchImpl,
        url: commitUrl,
        deadline: args.deadline,
        operation: "commit",
        useNodeTransport: args.useNodeTransport,
        ...(args.hasRunAcceptedBootstrap ? { hasRunAcceptedBootstrap: args.hasRunAcceptedBootstrap } : {}),
        ...(args.signal ? { signal: args.signal } : {}),
        init: {
          method: "POST",
          headers: {
            authorization: `Bearer ${args.token}`,
            "content-type": "application/json",
            accept: "application/json",
            ...(args.target.routingHeaders ?? {})
          },
          body: JSON.stringify({
            inputs: args.inputs.map((input) => ({
              inputId: input.inputId,
              sha256: input.sha256,
              sizeBytes: input.sizeBytes
            }))
          })
        }
      });
    } catch (err) {
      if (args.signal?.aborted) throw err;
      lastError = bootstrapNetworkError("commit", err);
      if (await checkRunAcceptedBootstrap(args.hasRunAcceptedBootstrap)) return;
      backoffMs = await waitForBootstrapRetry(backoffMs, args.deadline, args.signal, lastError);
      continue;
    }
    if (res.ok) return;
    const detail = await res.text().catch(() => "");
    const err = new Error(
      `AgentExecutor.submitRun: bootstrap commit failed with ${res.status}` +
        (detail ? `: ${detail.slice(0, 300)}` : "")
    );
    if (!DIRECT_BOOTSTRAP_RETRY_STATUSES.has(res.status)) {
      throw err;
    }
    lastError = err;
    if (await checkRunAcceptedBootstrap(args.hasRunAcceptedBootstrap)) return;
    backoffMs = await waitForBootstrapRetry(backoffMs, args.deadline, args.signal, lastError);
  }
  throw lastError ?? new Error("AgentExecutor.submitRun: aborted");
}

async function checkRunAcceptedBootstrap(check: (() => Promise<boolean>) | undefined): Promise<boolean> {
  if (!check) return false;
  return await check().catch(() => false);
}

function hasRunAcceptedBootstrap(run: Run): boolean {
  if (run.status === "succeeded" || run.status === "cancelled" || run.status === "timed_out") return true;
  if (run.status === "failed") return run.terminalAt !== undefined && run.terminalAt !== null;
  return run.status === "running" || run.status === "provider_running";
}

async function abortDirectBootstrap(args: {
  readonly fetchImpl: FetchLike;
  readonly token: string;
  readonly statusUrl: string;
  readonly target?: DirectBootstrapReady;
  readonly signal?: AbortSignal;
}): Promise<void> {
  const abortUrl = args.target?.abortUrl ?? `${args.statusUrl.replace(/\/status$/, "")}/abort`;
  await args.fetchImpl(abortUrl, {
    method: "POST",
    headers: {
      authorization: `Bearer ${args.token}`,
      accept: "application/json",
      ...(args.target?.routingHeaders ?? {})
    },
    ...(args.signal ? { signal: args.signal } : {})
  });
}

function bootstrapDeadline(expiresAt: string | undefined): number {
  if (typeof expiresAt === "string") {
    const parsed = Date.parse(expiresAt);
    if (Number.isFinite(parsed)) return parsed;
  }
  return Date.now() + 60_000;
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return Boolean(
    value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      Object.values(value as Record<string, unknown>).every((entry) => typeof entry === "string")
  );
}

function stripTrailingSlash(s: string): string {
  return s.endsWith("/") ? s.slice(0, -1) : s;
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
      throw new Error(`AgentExecutor.submitRun: mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw new Error(
          `AgentExecutor.submitRun: mcpServers[${i}].url conflicts with secrets.mcpServers["${secret.name}"]`
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
        `AgentExecutor.submitRun: proxyEndpoint "${entry.name}" auth type conflicts ` +
          `with secrets.proxyEndpointAuth (instance=${entry.value.type}, secrets=${existing.value.type})`
      );
    }
    byName.set(entry.name, entry);
  }
  return Array.from(byName.values());
}

// Side-channel re-exports keep the proxy wire types reachable from
// `import type { … } from "aex/client"` without forcing consumers
// to learn an additional entry point.
export type { PlatformProxyEndpoint, PlatformProxyEndpointAuth };
