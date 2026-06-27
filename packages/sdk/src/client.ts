import {
  AexError,
  DEFAULT_CREDENTIAL_MODE,
  DEFAULT_RUN_PROVIDER,
  HttpClient,
  REGIONS,
  RUNTIME_KINDS,
  RunConfigValidationError,
  RunStateError,
  SecretString,
  isRunSettled,
  operations,
  parseCredentialMode,
  providersForModel,
  streamCoordinatorEvents,
  summarizeRunTrace,
  textOf,
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
  type OutputFileType,
  type OutputLink,
  type OutputLinkOptions,
  type OutputQuery,
  type OutputText,
  type OutputMode,
  type ReadOutputTextOptions,
  type RunListPage,
  type RunListQuery,
  type PlatformEnvironmentInput,
  type PlatformRunSubmissionInput,
  type PlatformSubmission,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type PlatformProxyEndpoint,
  type PlatformProxyEndpointAuth,
  type PlatformPostHookInput,
  type Run,
  type RunModel,
  type RunEvent,
  type RunTrace,
  type UsageSummary,
  type RunLimits,
  parseRunLimits,
  type RunWebhookDelivery,
  type RunProvider,
  type Region,
  type SecretRecord,
  type SecretReveal,
  type RunUnit,
  BUILTIN_TOOL_NAMES,
  type BuiltinToolName,
  type RuntimeSize,
  type RuntimeKind,
  type Skill as SkillRecord,
  type SkillRef,
  type ToolRef,
  type WhoAmI,
  TERMINAL_RUN_STATUSES
} from "@aexhq/contracts";
import { AgentsMd } from "./agents-md.js";
import { uploadAsset, type AssetFetch, type UploadedAsset } from "./asset-upload.js";
import { File } from "./file.js";
import { McpServer } from "./mcp-server.js";
import { ProxyEndpoint, splitProxyEndpoints } from "./proxy-endpoint.js";
import { Secret, splitSecretEnv } from "./secret.js";
import { Skill } from "./skill.js";
import { Tool } from "./tool.js";

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
 *   - `apiKey` / `credentials` / `secrets` — the BYOK provider key(s). A key for
 *     the selected provider is REQUIRED; the simplest call passes just `apiKey`.
 *     Use `credentials` (or `secrets.apiKeys`) to carry keys for additional
 *     providers so subagents spawned with a different-family model can use them
 *     (the child inherits the parent's keys server-side). The platform never
 *     holds a long-lived provider key on your behalf.
 *
 * `idempotencyKey` is auto-generated when omitted; pass one explicitly
 * if you want client-driven retry safety across process restarts.
 */
export interface SubmitOptions {
  /**
   * Credential source for upstream provider access. Omitted defaults to
   * `"byok"`, which requires `secrets.apiKey` or
   * `secrets.apiKeys[provider]`.
   */
  readonly credentialMode?: CredentialMode;
  /**
   * Upstream provider selector. Prefer naming it explicitly with the
   * {@link Providers} symbol const, e.g. `provider: Providers.DEEPSEEK`. The
   * same model id can route through different providers, so `provider` is a
   * first-class field — pass it alongside `model` rather than letting the model
   * alone decide routing. The BYOK key for the selected provider is supplied as
   * `secrets.apiKey` or `secrets.apiKeys[provider]`.
   *
   * Optional today: when omitted it is derived from `model` (each currently
   * supported model maps to a single provider), so existing call sites keep
   * working. If supplied it MUST match the model's provider or `submit`
   * throws.
   */
  readonly provider?: RunProvider;
  /**
   * Optional runtime selector. Omit it or pass `"managed"`; both run on
   * the managed runtime through the hosted BYOK provider-proxy. `"native"`
   * is no longer accepted.
   */
  readonly runtime?: RuntimeKind;
  /**
   * Optional hosted-platform placement region for this run. These are
   * product-level tokens, not exact city guarantees; omit to let the platform
   * infer a configured region and fall back when no hint matches.
   */
  readonly region?: Region;
  /**
   * Closed public model id. Prefer the {@link Models} symbol const, e.g.
   * `Models.CLAUDE_HAIKU_4_5`. Pair it with an explicit {@link Providers} value
   * on `provider`; if `provider` is omitted it is derived from this model.
   */
  readonly model: RunModel;
  readonly system?: string;
  readonly prompt: string | readonly string[];
  readonly skills?: readonly Skill[];
  /**
   * Tools available to the agent. Each entry is either a custom {@link Tool}
   * bundle, or a BUILTIN tool reference — a bare name string, preferably
   * `BuiltinTools.<name>` (e.g. `BuiltinTools.notebook_edit`) so a typo is a
   * compile error. Builtin references compose with {@link includeBuiltinTools}:
   * use them to cherry-pick a tool the default set omits (notebook editing), or
   * to pick a narrow subset alongside `includeBuiltinTools: false`.
   *
   * Order in the agent's tool list: resolved builtin tools, then custom tools,
   * then MCP tools.
   */
  readonly tools?: readonly (Tool | BuiltinToolName)[];
  readonly agentsMd?: readonly AgentsMd[];
  readonly files?: readonly File[];
  readonly mcpServers?: readonly McpServer[];
  /**
   * Env-var secrets, keyed by env name. Each value is a {@link Secret}:
   * `Secret.value(v)` (ephemeral per-run — vaulted at submit, deleted at the
   * run's terminal) or `Secret.ref(handle)` (a persisted workspace secret,
   * resolved server-side). The SDK splits these into value-free declarations on
   * the hashed submission and ephemeral values into the vaulted secrets channel,
   * so a value never enters the run snapshot or the idempotency hash. The
   * runtime injects each as the named env var.
   */
  readonly secretEnv?: Readonly<Record<string, Secret>>;
  readonly environment?: PlatformEnvironmentInput;
  readonly metadata?: PlatformSubmission["metadata"];
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens.
   * Prefer the {@link RuntimeSizes} symbol const, e.g.
   * `RuntimeSizes.SHARED_2X_8GB`.
   */
  readonly runtimeSize?: RuntimeSize;
  /**
   * Run deadline as a duration string (`"1h"`, `"90m"`, `"30s"`). Bounded to
   * [1m, 6h]; omit for the 1h default. Applies to both runtimes.
   */
  readonly timeout?: string;
  /**
   * Command to run after the agent process exits successfully. A non-zero exit
   * or timeout is sent back to the model as a repair prompt until `maxTurns`
   * is exhausted. Empty commands are treated as omitted.
   */
  readonly postHook?: PlatformPostHookInput;
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
    readonly captureTimeoutMs?: number;
    readonly maxFileBytes?: number;
    readonly maxTotalBytes?: number;
    readonly maxFiles?: number;
  };
  /**
   * Whether to inject the standard builtin tool set
   * ({@link DEFAULT_BUILTIN_TOOLS} — every builtin except `notebook_edit`).
   *
   * - Omitted / `true` (default): inject the standard builtins.
   * - `false`: inject NO builtins — useful for a pure-MCP / pure-custom run.
   *   Cherry-pick a narrow subset back by listing builtin names in `tools`.
   */
  readonly includeBuiltinTools?: boolean;
  /**
   * Assistant-output granularity. `"buffered"` (default) delivers one event per
   * assistant message; `"stream"` delivers per-token text deltas for live
   * typing UIs.
   */
  readonly outputMode?: OutputMode;
  /**
   * Single-provider BYOK key sugar — the key for the run's selected `provider`.
   * The simplest call passes just `apiKey` (no nested `secrets` envelope). When
   * several sources name a key for the same provider they must agree (else submit
   * throws); resolution precedence is
   * `secrets.apiKeys[provider] ?? secrets.apiKey ?? credentials[provider] ?? apiKey`.
   */
  readonly apiKey?: string;
  /**
   * Multi-provider BYOK key map — the clean way to supply keys for more than one
   * provider (e.g. so a subagent spawned with a different-family model inherits a
   * key server-side). Folded into the `secrets.apiKeys` wire shape.
   */
  readonly credentials?: Partial<Record<RunProvider, string>>;
  /**
   * Advanced inline secrets bundle (per-provider `apiKeys`, MCP headers, proxy
   * auth, env secrets). OPTIONAL — the common case uses `apiKey` / `credentials`.
   * `secrets.apiKey` / `secrets.apiKeys` keep working unchanged.
   */
  readonly secrets?: PlatformInlineSecrets;
  readonly idempotencyKey?: string;
  /**
   * Lineage parent (agent-session §9). When set, the server admits this run as
   * a CHILD of `parentRunId` (same workspace required), enforcing the
   * max-subagent-depth + per-root concurrency caps and persisting the lineage.
   * The depth is always derived server-side from the parent row — clients name
   * the parent, never the depth.
   */
  readonly parentRunId?: string;
  /**
   * Optional per-run callback URL. The platform delivers exactly the terminal
   * `run.finished` event to `webhook.url` at the settle-consistent barrier,
   * signed Standard-Webhooks style (verify with {@link verifyAexWebhook}). The
   * URL must be https. It rides alongside `idempotencyKey` and never enters the
   * idempotency hash, so re-submitting the same key with a different callback
   * URL does not 409 (the URL is bound at first accept only).
   */
  readonly webhook?: { readonly url: string };
  /**
   * Optional per-run override of the lineage limits — the max number of
   * concurrent child runs and the max subagent depth. A sibling of
   * {@link parentRunId}: these are dials the client *requests*; the server
   * resolves each against the per-workspace ceiling and the hard platform
   * ceiling, and an absent field falls back to the platform default. Only
   * shape + positivity are validated client-side.
   */
  readonly limits?: RunLimits;
  readonly signal?: AbortSignal;
}

/** @deprecated Renamed to {@link SubmitOptions}. Kept for one release. */
export type SubmitRunOptions = SubmitOptions;

/**
 * The settle-consistent result of {@link AgentExecutor.run} / `runAndCollect`:
 * the terminal run record plus its settle-bracketed events, decoded trace,
 * assistant text, and captured outputs — everything a "do it and give me the
 * result" caller needs without hand-rolling a poll loop.
 */
export interface RunResult {
  readonly runId: string;
  /** The full terminal run record (status, costTelemetry, timings). */
  readonly run: Run;
  readonly status: string;
  /** `true` when `status === "succeeded"`. */
  readonly ok: boolean;
  /** The assistant's final text (decoded over the settled events). */
  readonly text: string;
  /** The settle-bracketed event stream (RUN_STARTED … terminal). */
  readonly events: readonly RunEvent[];
  /** Decoded view of the events: tool calls + usage + assistant text. */
  readonly trace: RunTrace;
  /** The run's captured output files. */
  readonly outputs: readonly Output[];
  /** Aggregate token usage when the deployment exposes it on the record. */
  readonly usage?: UsageSummary;
  /** Settle-time showback estimate (USD), from `run.costTelemetry`. */
  readonly costUsd?: number;
  /** The run's error message when `!ok`. */
  readonly error?: string;
}

/** Options for {@link AgentExecutor.run} / `runAndCollect`. */
export interface RunCollectOptions {
  /** Overall wait budget (ms) for the run to reach a terminal record. */
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
  /** Throw a {@link RunStateError} when the run does not succeed. Default false. */
  readonly throwOnFailure?: boolean;
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
  /**
   * End the stream settle-consistently. By default the iterator ends on the
   * AG-UI terminal event (RUN_FINISHED / RUN_ERROR) — the render-complete UX
   * signal, which the runner emits BEFORE the platform commits the run record,
   * so a `getRun` immediately after can still read `running`. With
   * `settleConsistent: true` the iterator keeps reading PAST the terminal event
   * until the post-mirror `aex.run.settled` barrier, so when it ends a
   * subsequent `getRun` is guaranteed terminal and `listOutputs` is complete.
   * Note: outputs are durable at the RUN_FINISHED event already; this only adds
   * the run-RECORD consistency barrier.
   */
  readonly settleConsistent?: boolean;
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

export type OutputLinkSelector = string | OutputFileSelector | OutputQuery;

export interface OutputDownloadOptions {
  readonly to?: string;
}

/**
 * Workspace skill admin operations exposed under `client.skills`.
 *
 * New run submissions usually use `Skill.fromFiles(...)` or
 * `Skill.fromPath(...)` directly inside `submit`; the SDK materializes
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
 * `AgentsMd.fromPath(...)` directly inside `submit`; the SDK
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
 * `File.fromBytes(...)` directly inside `submit`; the SDK materializes
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
 * Workspace secret management exposed under `client.secrets`, mirroring
 * `client.skills` / `client.files`.
 *
 * Lifecycle parity with assets/skills: a `Secret.value(...)` is per-run and
 * gone at terminal; `set` (or promoting an ephemeral via `secret.upload`)
 * persists a named, searchable workspace secret you can `get` (metadata),
 * `get_value` (audited value), `rotate`, `list`, and `delete`. The identity is the
 * `name`; the value rotates under that stable name.
 *
 * Values are write-only: `set`/`rotate` send the value in the request BODY (never
 * the URL); `get`/`list` return metadata only; `get_value` is the explicit audited
 * value read.
 */
export class SecretsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  /** Create a named workspace secret. Accepts a raw string or a `SecretString`. */
  set(args: { readonly name: string; readonly value: string | SecretString }): Promise<SecretRecord> {
    return operations.createSecret(this.#http, { name: args.name, value: unwrapSecretValue(args.value) });
  }

  /** List workspace secret metadata (searchable by name). Never returns values. */
  list(): Promise<readonly SecretRecord[]> {
    return operations.listSecrets(this.#http);
  }

  /** Metadata for one workspace secret by name. Never returns the value. */
  get(name: string): Promise<SecretRecord> {
    return operations.getSecret(this.#http, name);
  }

  /** Audited value read — the preferred path that returns a workspace secret value. */
  get_value(name: string): Promise<SecretReveal> {
    return operations.getSecretValue(this.#http, name);
  }

  /** Replace the value of an existing workspace secret; bumps its version. */
  rotate(args: { readonly name: string; readonly value: string | SecretString }): Promise<SecretRecord> {
    return operations.rotateSecret(this.#http, { name: args.name, value: unwrapSecretValue(args.value) });
  }

  delete(name: string): Promise<void> {
    return operations.deleteSecret(this.#http, name);
  }

  /**
   * Internal: create a workspace secret from an ephemeral `Secret.value(...)`
   * being promoted via `secret.upload(client, { name })`. NOT part of the
   * public API — callers use `set`.
   */
  async _createWorkspaceSecret(args: { readonly name: string; readonly value: string }): Promise<{ readonly name: string }> {
    const record = await operations.createSecret(this.#http, args);
    return { name: record.name };
  }
}

/** Accept a raw string or a `SecretString`; return the raw value for the wire. */
function unwrapSecretValue(value: string | SecretString): string {
  const raw = value instanceof SecretString ? value.unwrap() : value;
  if (typeof raw !== "string" || !raw) {
    throw new Error("secrets: value must be a non-empty string");
  }
  return raw;
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
  /** The same fetch the HttpClient uses, threaded into `_uploadAsset`. */
  readonly #fetch: FetchLike | undefined;
  readonly skills: SkillsClient;
  readonly agentsMd: AgentsMdClient;
  readonly files: FilesClient;
  readonly secrets: SecretsClient;

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
    this.secrets = new SecretsClient(this.#http);
  }

  /**
   * Internal: forwards to `SkillsClient._uploadSkillBundle`. NOT part of
   * the public API.
   *
   * NOTE (tech-debt): this is part of the legacy workspace-skill upload
   * surface (`SkillsClient` + `operations.createSkillBundle` + the TUS
   * chunked path in asset-upload.ts). The live submit path materializes
   * inline skills via `uploadAsset` instead; `Skill.upload(client)`
   * pre-stages a draft explicitly for reuse. This surface is retained
   * pending a deliberate deprecation pass (it still threads into the CLI
   * host commands), tracked in the remediation plan as item 4a.
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
   * Internal: satisfies the `SecretUploader` surface so a
   * `Secret.value(...).upload(client, { name })` promotes an ephemeral secret
   * into the workspace store. Forwarded to `SecretsClient._createWorkspaceSecret`.
   * NOT part of the public API.
   */
  async _createWorkspaceSecret(args: { readonly name: string; readonly value: string }): Promise<{ readonly name: string }> {
    return this.secrets._createWorkspaceSecret(args);
  }

  /**
   * Internal: materialize raw bytes to the content-addressable asset store
   * (`/assets/presign` → PUT → `/assets/finalize`). Used by `Skill.upload(this)`
   * to pre-upload a draft skill bundle so a later run carries only a plain
   * `kind:"asset"` ref. NOT part of the public API.
   */
  async _uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<UploadedAsset> {
    return uploadAsset({
      http: this.#http,
      bytes: args.bytes,
      hash: args.hash,
      ...(args.contentType ? { contentType: args.contentType } : {}),
      ...(this.#fetch ? { fetch: this.#fetch as unknown as AssetFetch } : {})
    });
  }

  /**
   * Submit a run, wait until its RECORD is terminal, and collect the full
   * {@link RunResult} — the settle-consistent "do it and give me the result"
   * primitive. Folds the poll loop every consumer hand-rolled into one call:
   * submit → {@link waitForRun} (polls `getRun`, NOT the earlier RUN_FINISHED
   * event) → poll `listEvents` until the snapshot is settle-bracketed
   * (RUN_STARTED + a terminal event present) → `listOutputs` → decode the trace
   * and assistant text. On resolve, `getRun`/`listOutputs` are guaranteed
   * consistent.
   *
   * Uses polling (portable across backends), NOT the coordinator WebSocket. By
   * default a failed run resolves with `ok: false` and a populated `error`; pass
   * `{ throwOnFailure: true }` to throw instead. For live events prefer `submit`
   * + `streamEnvelopes(runId, { settleConsistent: true })`.
   */
  async run(options: SubmitOptions, opts: RunCollectOptions = {}): Promise<RunResult> {
    const signal = opts.signal ?? options.signal;
    const runId = await this.submit(options);
    const run = await this.waitForRun(runId, {
      ...(opts.timeoutMs !== undefined ? { timeoutMs: opts.timeoutMs } : {}),
      ...(signal ? { signal } : {})
    });
    const events = await this.#collectSettledEvents(runId, signal);
    const outputs = await this.listOutputs(runId);
    const ok = run.status === "succeeded";
    const costUsd = run.costTelemetry?.billedCostUsd;
    const errorMessage = typeof run.errorMessage === "string" && run.errorMessage ? run.errorMessage : undefined;
    const result: RunResult = {
      runId,
      run,
      status: run.status,
      ok,
      text: textOf(events),
      events,
      trace: summarizeRunTrace(events),
      outputs,
      ...(run.usage ? { usage: run.usage } : {}),
      ...(typeof costUsd === "number" ? { costUsd } : {}),
      ...(!ok && errorMessage ? { error: errorMessage } : {})
    };
    if (opts.throwOnFailure && !ok) {
      throw new RunStateError(
        `AgentExecutor.run: run ${runId} ended ${run.status}${errorMessage ? `: ${errorMessage}` : ""}`,
        { runId, status: run.status }
      );
    }
    return result;
  }

  /**
   * Explicit, discoverable alias for {@link run}: submit, wait, and collect the
   * full {@link RunResult} in one call.
   */
  runAndCollect(options: SubmitOptions, opts?: RunCollectOptions): Promise<RunResult> {
    return this.run(options, opts);
  }

  /**
   * Poll `listEvents` until the snapshot is settle-bracketed — both a
   * RUN_STARTED and a terminal (RUN_FINISHED / RUN_ERROR) event present — then
   * return it. The runner emits the terminal AG-UI event BEFORE the platform
   * commits the record, and the `listEvents` snapshot can lag the terminal
   * record by a beat; this closes that race so the decoded trace/text/outputs
   * are complete. Bounded so an older runtime that never emits one of the
   * brackets still returns the best snapshot available.
   */
  async #collectSettledEvents(runId: string, signal: AbortSignal | undefined): Promise<readonly RunEvent[]> {
    const intervalMs = 500;
    const maxAttempts = 20;
    let latest: readonly RunEvent[] = [];
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      if (signal?.aborted) return latest;
      latest = await this.listEvents(runId);
      const hasStart = latest.some((event) => event.type === "RUN_STARTED");
      const hasTerminal = latest.some((event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR");
      if (hasStart && hasTerminal) return latest;
      try {
        await sleep(intervalMs, signal);
      } catch {
        return latest;
      }
    }
    return latest;
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
   * Unstaged inline skills / agentsMd / files (`Skill.fromFiles` /
   * `Skill.fromPath` / `AgentsMd.fromContent` / `File.fromBytes` without a
   * prior `.upload`) are auto-uploaded to the content-addressable asset
   * store (`/assets/presign` → PUT → `/assets/finalize`) before `POST /runs`,
   * deduped by content hash, and referenced in the submission as plain
   * `{ kind:"asset" }` refs — identical to a pre-staged `.upload(client)`.
   */
  async submit(options: SubmitOptions): Promise<string> {
    if (!options || typeof options !== "object") {
      throw new RunConfigValidationError("AgentExecutor.submit: options is required");
    }
    // A model maps to one or more upstream providers (see MODEL_PROVIDER_IDS).
    // `providersForModel` returns the supported providers in priority order, or
    // `[]` for an unknown model string (the model check below then rejects it).
    // An explicit provider is allowed but must be one that serves the model;
    // when omitted the model's default (first-listed) provider is used.
    const supportedProviders = providersForModel(options.model);
    if (
      options.provider &&
      supportedProviders.length > 0 &&
      !supportedProviders.includes(options.provider)
    ) {
      throw new RunConfigValidationError(
        `AgentExecutor.submit: provider ${JSON.stringify(options.provider)} is not available for ` +
          `model ${JSON.stringify(options.model)} (supported: ${supportedProviders.join(", ")})`
      );
    }
    const provider: RunProvider = options.provider ?? supportedProviders[0] ?? DEFAULT_RUN_PROVIDER;
    const credentialMode = parseCredentialMode(options.credentialMode);
    // Resolve the BYOK key(s) across the top-level sugar (`apiKey`,
    // `credentials`) and the advanced `secrets` bundle, folding everything into a
    // single `apiKeys` map (the wire shape the platform reads). Validates the
    // selected provider's key is present and that the sources don't disagree,
    // failing synchronously before any network call.
    const { foldedApiKeys } = resolveSubmitCredentials(options, provider);
    if (typeof options.model !== "string" || !options.model) {
      throw new RunConfigValidationError("AgentExecutor.submit: model is required");
    }
    const prompt = normalisePrompt(options.prompt);
    const { endpoints: proxyEndpointDeclarations, auth: proxyEndpointAuthFromInstances } =
      splitProxyEndpoints(options.proxyEndpoints ?? []);
    const mergedProxyAuth = mergeProxyEndpointAuth(
      proxyEndpointAuthFromInstances,
      options.secrets?.proxyEndpointAuth ?? []
    );
    // Split secretEnv into value-free declarations (hashed submission) and
    // ephemeral values (vaulted secrets channel), mirroring the proxy split.
    const { declarations: secretEnvDeclarations, values: envSecretValues } =
      splitSecretEnv(options.secretEnv);

    // Validate the runtime selector before any network I/O — inline drafts
    // are uploaded below, so an invalid runtime must reject first rather than
    // leak an asset upload.
    if (
      options.runtime !== undefined &&
      !(RUNTIME_KINDS as readonly string[]).includes(options.runtime)
    ) {
      throw new AexError(
        "RUNTIME_UNSUPPORTED",
        `AgentExecutor.submit: runtime must be one of: ${RUNTIME_KINDS.join(", ")} ` +
          `(got ${JSON.stringify(options.runtime)})`
      );
    }
    if (
      options.region !== undefined &&
      !(REGIONS as readonly string[]).includes(options.region)
    ) {
      throw new AexError(
        "RUN_CONFIG_INVALID",
        `AgentExecutor.submit: region must be one of: ${REGIONS.join(", ")} ` +
          `(got ${JSON.stringify(options.region)})`
      );
    }
    // Validate the per-run limits override with the SAME parser the server runs
    // (shape + positivity + allow-list), failing fast before any asset upload.
    // Normalizes an all-absent override (e.g. `{}`) away.
    let limits: RunLimits | undefined;
    try {
      limits = parseRunLimits(options.limits);
    } catch (err) {
      throw new AexError(
        "RUN_CONFIG_INVALID",
        `AgentExecutor.submit: ${err instanceof Error ? err.message : String(err)}`
      );
    }

    // Walk Skill / Tool / AgentsMd / File instances. Inline drafts are eagerly
    // uploaded to the content-addressable asset store here (before POST /runs)
    // and referenced as plain `kind:"asset"` refs. Already-materialized asset
    // refs pass through unchanged.
    const uploader: AssetUploader = (args) => this._uploadAsset(args);
    const preparedSkills = await prepareSkills(options.skills ?? [], uploader);
    const preparedTools = await prepareTools(options.tools ?? [], uploader);
    const preparedAgentsMd = await prepareAgentsMd(options.agentsMd ?? [], uploader);
    const preparedFiles = await prepareFiles(options.files ?? [], uploader);
    const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(
      options.mcpServers ?? [],
      options.secrets?.mcpServers ?? []
    );
    const outputCapture = outputsForWire(options.outputs);

    const submission: PlatformSubmission = {
      model: options.model,
      ...(options.system ? { system: options.system } : {}),
      prompt,
      skills: preparedSkills,
      // The wire `tools` is the union: builtin name strings (cherry-picks)
      // followed by the custom tool bundle refs. The shared parser splits them
      // back into `tools` (custom) + `builtinTools` (names). The cast
      // acknowledges the SDK is producing pre-parse wire input here, same as
      // `mcpServers` / `environment` below.
      tools: [...preparedTools.builtinNames, ...preparedTools.refs] as unknown as readonly ToolRef[],
      agentsMd: preparedAgentsMd,
      files: preparedFiles,
      // submissionMcpServers may contain workspace refs of the shape
      // {kind:"workspace", id:"mcp_..."}. The BFF runs
      // `resolveWorkspaceMcpRefsInSubmission` BEFORE the shared parser
      // and replaces them with the resolved {name, url}, so by the
      // time anything reads PlatformSubmission post-parse the
      // shape matches McpServerRef. The cast acknowledges that the
      // SDK is producing pre-resolution wire input here.
      mcpServers: submissionMcpServers as readonly McpServerRef[],
      ...(Object.keys(secretEnvDeclarations).length > 0 ? { secretEnv: secretEnvDeclarations } : {}),
      // `options.environment.packages` carry the customer wire shape
      // (`{name:"pip:pandas"}`); the shared parser resolves the ecosystem
      // prefix into PlatformPackage. The cast acknowledges the SDK is
      // producing pre-parse wire input here, same as `mcpServers` above.
      ...(options.environment
        ? { environment: options.environment as NonNullable<PlatformSubmission["environment"]> }
        : {}),
      ...(options.metadata ? { metadata: options.metadata } : {}),
      ...(outputCapture ? { outputs: outputCapture } : {}),
      // Pass-through the builtin-tool toggle verbatim (omitted ⇒ default ON).
      ...(options.includeBuiltinTools !== undefined
        ? { includeBuiltinTools: options.includeBuiltinTools }
        : {}),
      ...(options.outputMode !== undefined ? { outputMode: options.outputMode } : {})
    };

    const secrets: PlatformInlineSecrets = {
      ...options.secrets,
      // Folded BYOK keys (sugar + secrets.apiKeys) override; omitted entirely
      // when empty so a pure legacy `secrets.apiKey` submission stays unchanged.
      ...(Object.keys(foldedApiKeys).length > 0 ? { apiKeys: foldedApiKeys } : {}),
      ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {}),
      ...(mergedProxyAuth.length > 0 ? { proxyEndpointAuth: mergedProxyAuth } : {}),
      ...(Object.keys(envSecretValues).length > 0 ? { envSecrets: envSecretValues } : {})
    };

    const postHook = postHookForWire(options.postHook);
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
      ...(options.region ? { region: options.region } : {}),
      submission,
      ...(options.runtimeSize ? { runtimeSize: options.runtimeSize } : {}),
      ...(options.timeout ? { timeout: options.timeout } : {}),
      ...(postHook ? { postHook } : {}),
      ...(options.parentRunId ? { parentRunId: options.parentRunId } : {}),
      // Operational/delivery concern — sibling of idempotencyKey, NOT part of
      // the hashed brief. The idempotency key here is randomly generated, so
      // including the field has no effect on dedup.
      ...(options.webhook ? { webhook: options.webhook } : {}),
      // Per-run lineage-limit override — a top-level operational dial (sibling
      // of parentRunId), NOT part of the hashed submission. Validated + normalized
      // above; the server re-clamps against the workspace + platform ceilings.
      ...(limits ? { limits } : {}),
      secrets,
      ...(proxyEndpointDeclarations.length > 0
        ? { proxyEndpoints: proxyEndpointDeclarations }
        : {})
    };

    const run = await operations.submitRun(this.#http, request);
    return getSubmittedRunId(run);
  }

  getRun(runId: string): Promise<Run> {
    return operations.getRun(this.#http, runId);
  }

  /** Short alias for `getRun`. */
  get(runId: string): Promise<Run> {
    return this.getRun(runId);
  }

  /**
   * List the runs in this workspace, most-recent first, one page at a time.
   * The workspace is derived server-side from the API token, so this only ever
   * enumerates your own runs. Pass `query.cursor` (from a prior page's
   * `nextCursor`) to page; omit it for the first page. Returns public-safe
   * {@link RunSummary} rows — the full submission stays behind `getRunUnit`.
   *
   * This is the workspace-wide discovery entry point: combine it with
   * `listOutputs` / `readOutputText` to reach any run's deliverables.
   */
  listRuns(query?: RunListQuery): Promise<RunListPage> {
    return operations.listRuns(this.#http, query);
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
      // settleConsistent ends the stream on the post-mirror barrier instead of the
      // earlier RUN_FINISHED UX signal, so "stream ended" ⇒ getRun is terminal.
      ...(options.settleConsistent ? { isTerminal: isRunSettled } : {}),
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

  listOutputs(runId: string, query?: OutputQuery): Promise<readonly Output[]> {
    return operations.listOutputs(this.#http, runId, query);
  }

  /** Short alias for `listOutputs`. */
  outputs(runId: string, query?: OutputQuery): Promise<readonly Output[]> {
    return this.listOutputs(runId, query);
  }

  findOutputs(runId: string, query: OutputQuery): Promise<readonly Output[]> {
    return operations.findOutputs(this.#http, runId, query);
  }

  findOutput(runId: string, query: OutputQuery): Promise<Output | null> {
    return operations.findOutput(this.#http, runId, query);
  }

  outputLink(runId: string, selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<OutputLink> {
    return operations.outputLink(this.#http, runId, selectorOrQuery, options);
  }

  createOutputLink(runId: string, selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<OutputLink> {
    return this.outputLink(runId, selectorOrQuery, options);
  }

  async fetchOutput(runId: string, selectorOrQuery: OutputLinkSelector, options?: OutputLinkOptions): Promise<Response> {
    const link = await this.outputLink(runId, selectorOrQuery, options);
    return (this.#fetch ?? fetch)(link.url);
  }

  /**
   * Read ONE output file as byte-capped, decoded UTF-8 text. Streams the file and
   * STOPS at `options.maxBytes` (default 50 KB, ceiling 10 MB), so a huge
   * deliverable never fully buffers — ideal for handing a run's output to an LLM
   * tool. Check `result.truncated` before treating the text as complete; pass
   * `options.grep` to keep only matching lines. Select by `{ path }` or `{ id }`,
   * same as `downloadOutput`.
   */
  readOutputText(
    runId: string,
    selector: OutputFileSelector,
    options?: ReadOutputTextOptions
  ): Promise<OutputText> {
    return operations.readOutputText(this.#http, runId, selector, options);
  }

  eventArchiveLink(runId: string, options?: OutputLinkOptions): Promise<OutputLink> {
    return operations.eventArchiveLink(this.#http, runId, options);
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
   * List a run's webhook delivery attempts (the per-run delivery ledger).
   * Empty when the run carried no `webhook` or has not yet terminated.
   */
  getRunWebhookDeliveries(runId: string): Promise<readonly RunWebhookDelivery[]> {
    return operations.getRunWebhookDeliveries(this.#http, runId);
  }

  /**
   * Manually re-trigger a run's webhook delivery: re-sends the frozen payload
   * with the SAME `webhook-id` so the consumer dedupes.
   */
  redeliverRunWebhook(runId: string, deliveryId: string): Promise<void> {
    return operations.redeliverRunWebhook(this.#http, runId, deliveryId);
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
      throw new RunConfigValidationError("AgentExecutor.submit: prompt must be a non-empty string");
    }
    return [input];
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new RunConfigValidationError("AgentExecutor.submit: prompt must be a non-empty string or string array");
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new RunConfigValidationError("AgentExecutor.submit: prompt segments must be non-empty strings");
    }
  }
  return [...input];
}

/**
 * Resolve the BYOK provider key(s) for a submission across the top-level sugar
 * (`apiKey`, `credentials`) and the advanced `secrets` bundle, folding them into
 * one `apiKeys` map (the wire shape the platform reads). Per-provider precedence:
 * `secrets.apiKeys[p] ?? secrets.apiKey ?? credentials[p] ?? apiKey`. Legacy
 * `secrets.apiKey` is left in place (not folded) so a pure-legacy submission's
 * wire shape is unchanged; the platform falls back to it for the selected
 * provider. Throws synchronously when the selected provider has no key, or when
 * the sources name DIFFERENT keys for it (a call-site mistake).
 */
function resolveSubmitCredentials(
  options: SubmitOptions,
  provider: RunProvider
): { foldedApiKeys: Partial<Record<RunProvider, string>> } {
  const secrets = options.secrets;
  const selectedCandidates = [
    secrets?.apiKeys?.[provider],
    secrets?.apiKey,
    options.credentials?.[provider],
    options.apiKey
  ].filter((value): value is string => typeof value === "string" && value.length > 0);
  if (new Set(selectedCandidates).size > 1) {
    throw new RunConfigValidationError(
      `AgentExecutor.submit: conflicting API keys for provider ${JSON.stringify(provider)} ` +
        "(secrets.apiKeys / secrets.apiKey / credentials / apiKey disagree). Supply exactly one."
    );
  }
  if (selectedCandidates.length === 0) {
    throw new RunConfigValidationError(
      "AgentExecutor.submit: a provider API key is required — pass `apiKey`, " +
        "`credentials[provider]`, `secrets.apiKey`, or `secrets.apiKeys[provider]`."
    );
  }
  // Fold in precedence order (lowest first; later writes win):
  // apiKey (selected provider) < credentials < secrets.apiKeys.
  const foldedApiKeys: Partial<Record<RunProvider, string>> = {};
  if (typeof options.apiKey === "string" && options.apiKey.length > 0) {
    foldedApiKeys[provider] = options.apiKey;
  }
  for (const [p, key] of Object.entries(options.credentials ?? {})) {
    if (typeof key === "string" && key.length > 0) {
      foldedApiKeys[p as RunProvider] = key;
    }
  }
  for (const [p, key] of Object.entries(secrets?.apiKeys ?? {})) {
    if (typeof key === "string" && key.length > 0) {
      foldedApiKeys[p as RunProvider] = key;
    }
  }
  return { foldedApiKeys };
}

function postHookForWire(input: PlatformPostHookInput | undefined): PlatformPostHookInput | undefined {
  if (input === undefined || typeof input.command !== "string" || input.command.trim().length === 0) {
    return undefined;
  }
  return input;
}

function outputsForWire(outputs: SubmitOptions["outputs"]): PlatformSubmission["outputs"] | undefined {
  if (outputs === undefined) {
    return undefined;
  }
  const allowedDirs = outputs.allowedDirs?.filter((dir) => dir.length > 0);
  const deniedDirs = outputs.deniedDirs?.filter((dir) => dir.length > 0);
  const hasNumericOverride =
    outputs.captureTimeoutMs !== undefined ||
    outputs.maxFileBytes !== undefined ||
    outputs.maxTotalBytes !== undefined ||
    outputs.maxFiles !== undefined;
  if ((allowedDirs?.length ?? 0) === 0 && (deniedDirs?.length ?? 0) === 0 && !hasNumericOverride) {
    return undefined;
  }
  return {
    ...(allowedDirs && allowedDirs.length > 0 ? { allowedDirs } : {}),
    ...(deniedDirs && deniedDirs.length > 0 ? { deniedDirs } : {}),
    ...(outputs.captureTimeoutMs !== undefined ? { captureTimeoutMs: outputs.captureTimeoutMs } : {}),
    ...(outputs.maxFileBytes !== undefined ? { maxFileBytes: outputs.maxFileBytes } : {}),
    ...(outputs.maxTotalBytes !== undefined ? { maxTotalBytes: outputs.maxTotalBytes } : {}),
    ...(outputs.maxFiles !== undefined ? { maxFiles: outputs.maxFiles } : {})
  };
}

/**
 * Stages a draft bundle's bytes to the content-addressable asset store and
 * returns the resulting asset id. Satisfied by `AgentExecutor._uploadAsset`.
 */
type AssetUploader = (args: {
  readonly bytes: Uint8Array;
  readonly hash: string;
  readonly contentType?: string;
}) => Promise<UploadedAsset>;

/**
 * A draft asset instance: yields its bytes once and caches the resolved asset id
 * so reuse across submits skips a re-upload (uploads are content-hash deduped).
 */
interface DraftAsset {
  readonly _cachedAssetId: string | undefined;
  _rememberAsset(assetId: string): void;
}

/**
 * Resolve a draft's asset id: reuse the cached id from a prior submit, otherwise
 * upload the bytes and cache the result on the instance.
 */
async function resolveAssetId(
  entry: DraftAsset,
  bundle: { readonly bytes: Uint8Array; readonly contentHash: string },
  uploader: AssetUploader
): Promise<string> {
  const cached = entry._cachedAssetId;
  if (cached !== undefined) {
    return cached;
  }
  const uploaded = await uploader({
    bytes: bundle.bytes,
    hash: bundle.contentHash,
    contentType: "application/zip"
  });
  entry._rememberAsset(uploaded.assetId);
  return uploaded.assetId;
}

/** Walk Skill[], eagerly upload drafts as assets, and return plain asset refs. */
async function prepareSkills(
  skills: readonly Skill[],
  uploader: AssetUploader
): Promise<readonly SkillRef[]> {
  const refs: SkillRef[] = [];
  for (let i = 0; i < skills.length; i++) {
    const entry = skills[i];
    if (!(entry instanceof Skill)) {
      throw new RunConfigValidationError(`AgentExecutor.submit: skills[${i}] must be a Skill instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`AgentExecutor.submit: skills[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({
        kind: "asset",
        assetId,
        name: bundle.name
      });
      continue;
    }
    // Already-materialized asset ref.
    refs.push(ref);
  }
  return refs;
}

/**
 * Split the `tools` union into custom tool refs (drafts eagerly uploaded as
 * assets) and builtin tool-name references (bare strings, validated against the
 * closed {@link BUILTIN_TOOL_NAMES} set). Builtin names are deduped, in input
 * order; the two groups are recombined on the wire (builtins first) by the
 * caller.
 */
async function prepareTools(
  tools: readonly (Tool | BuiltinToolName)[],
  uploader: AssetUploader
): Promise<{ readonly refs: readonly ToolRef[]; readonly builtinNames: readonly BuiltinToolName[] }> {
  const refs: ToolRef[] = [];
  const seenBuiltins = new Set<BuiltinToolName>();
  const builtinNames: BuiltinToolName[] = [];
  for (let i = 0; i < tools.length; i++) {
    const entry = tools[i];
    // A bare string is a builtin tool reference.
    if (typeof entry === "string") {
      if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)) {
        throw new RunConfigValidationError(
          `AgentExecutor.submit: tools[${i}] (${JSON.stringify(entry)}) is not a builtin tool name; ` +
            `expected a Tool instance or one of: ${BUILTIN_TOOL_NAMES.join(", ")}`
        );
      }
      if (!seenBuiltins.has(entry)) {
        seenBuiltins.add(entry);
        builtinNames.push(entry);
      }
      continue;
    }
    if (!(entry instanceof Tool)) {
      throw new RunConfigValidationError(`AgentExecutor.submit: tools[${i}] must be a Tool instance or a builtin tool name`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`AgentExecutor.submit: tools[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({ ...bundle.ref, assetId });
      continue;
    }
    refs.push(ref);
  }
  return { refs, builtinNames };
}

/** Walk AgentsMd[], eagerly upload drafts as assets, and return plain asset refs. */
async function prepareAgentsMd(
  agentsMds: readonly AgentsMd[],
  uploader: AssetUploader
): Promise<readonly AgentsMdRef[]> {
  const refs: AgentsMdRef[] = [];
  for (let i = 0; i < agentsMds.length; i++) {
    const entry = agentsMds[i];
    if (!(entry instanceof AgentsMd)) {
      throw new RunConfigValidationError(`AgentExecutor.submit: agentsMd[${i}] must be an AgentsMd instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`AgentExecutor.submit: agentsMd[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({
        kind: "asset",
        assetId,
        name: bundle.name
      });
      continue;
    }
    refs.push(ref);
  }
  return refs;
}

/** Walk File[], eagerly upload drafts as assets, and return plain asset refs. */
async function prepareFiles(
  files: readonly File[],
  uploader: AssetUploader
): Promise<readonly FileRef[]> {
  const refs: FileRef[] = [];
  for (let i = 0; i < files.length; i++) {
    const entry = files[i];
    if (!(entry instanceof File)) {
      throw new RunConfigValidationError(`AgentExecutor.submit: files[${i}] must be a File instance`);
    }
    const ref = entry.ref;
    if (ref.kind === "draft") {
      const bundle = entry._takeDraftBundle();
      if (!bundle) {
        throw new RunConfigValidationError(`AgentExecutor.submit: files[${i}] is draft but has no bytes`);
      }
      const assetId = await resolveAssetId(entry, bundle, uploader);
      refs.push({
        kind: "asset",
        assetId,
        name: bundle.name,
        mountPath: bundle.mountPath
      });
      continue;
    }
    refs.push(ref);
  }
  return refs;
}

function getSubmittedRunId(response: { readonly id?: string; readonly runId?: string }): string {
  const id = response.id ?? response.runId;
  if (typeof id !== "string" || id.length === 0) {
    throw new RunStateError("AgentExecutor.submit: submit response did not include a run id");
  }
  return id;
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
      throw new RunConfigValidationError(`AgentExecutor.submit: mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw new RunConfigValidationError(
          `AgentExecutor.submit: mcpServers[${i}].url conflicts with secrets.mcpServers["${secret.name}"]`
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
      throw new RunConfigValidationError(
        `AgentExecutor.submit: proxyEndpoint "${entry.name}" auth type conflicts ` +
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
export type { OutputFileType, OutputLink, OutputLinkOptions, OutputQuery, PlatformProxyEndpoint, PlatformProxyEndpointAuth };
