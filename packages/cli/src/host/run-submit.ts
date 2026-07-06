import { zipSync, type Zippable } from "fflate";
import {
  BUILTIN_TOOL_NAMES,
  DEFAULT_FILE_MOUNT_PATH,
  SKILL_BUNDLE_LIMITS,
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  SKILLS_MAX,
  TOOL_NAME_PATTERN,
  normaliseSkillBundlePath,
  operations,
  parseRunLimits,
  parseRunTimeout,
  parseRunWebhook,
  resolveModelProvider,
  validateSkillBundleEntry,
  type AgentsMdRef,
  type BuiltinToolName,
  type FetchLike,
  type FileRef,
  type HttpClient,
  type JsonValue,
  type McpServerRef,
  type PlatformEnvironment,
  type PlatformEnvironmentInput,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type PlatformSubmission,
  type RunLimits,
  type RunModel,
  type RunProvider,
  type RuntimeSize,
  type Session,
  type SessionCreateRequest,
  type ToolInputSchema,
  type ToolRef
} from "@aexhq/contracts";

const TEXT = new TextEncoder();
const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
const UPLOAD_CONCURRENCY = 5;

export interface CliSkillDraft {
  readonly name: string;
  readonly description: string;
  readonly contentHash: string;
  readonly bytes: Uint8Array;
}

export interface CliToolDraft {
  readonly ref: Omit<ToolRef, "assetId"> & { readonly contentHash: string };
  readonly bytes: Uint8Array;
}

export interface CliAgentsMdDraft {
  readonly name: string;
  readonly contentHash: string;
  readonly bytes: Uint8Array;
}

export interface CliFileDraft {
  readonly name: string;
  readonly contentHash: string;
  readonly mountPath: string;
  readonly bytes: Uint8Array;
}

export interface CliMcpServer {
  readonly name: string;
  readonly url: string;
  readonly headers?: Readonly<Record<string, string>>;
}

export interface CliRunSubmitOptions {
  readonly message: string | readonly string[];
  readonly provider?: RunProvider;
  readonly model: RunModel;
  readonly system?: string;
  readonly tools?: readonly (CliToolDraft | BuiltinToolName)[];
  readonly skills?: readonly CliSkillDraft[];
  readonly agentsMd?: readonly CliAgentsMdDraft[];
  readonly files?: readonly CliFileDraft[];
  readonly mcpServers?: readonly CliMcpServer[];
  readonly metadata?: Readonly<Record<string, JsonValue>>;
  readonly apiKeys?: Partial<Record<RunProvider, string>>;
  readonly environment?: CliSessionEnvironmentOptions;
  readonly runtime?: RuntimeSize;
  readonly overrides?: {
    readonly idleTtl?: string;
    readonly timeout?: string;
    readonly maxSpendUsd?: number;
    readonly maxTurns?: number;
  };
  readonly webhook?: { readonly url: string };
  readonly idempotencyKey?: string;
}

export interface CliSessionEnvironmentOptions extends Omit<PlatformEnvironmentInput, "envVars"> {
  readonly variables?: Readonly<Record<string, string>>;
}

export async function submitCliRun(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  options: CliRunSubmitOptions
): Promise<Session> {
  const request = await buildSessionCreateRequest(http, fetchImpl, options);
  const submitted = await operations.submit(
    http,
    request,
    { idempotencyKey: operations.resolveIdempotencyKey(options.idempotencyKey) }
  );
  return submitted.session;
}

export async function buildCliSkill(content: string, source: string): Promise<CliSkillDraft> {
  const front = extractSkillFrontmatter("Skill.fromContent", { "SKILL.md": content });
  const name = deriveSkillName("Skill.fromContent", front.name, undefined, undefined);
  const description = front.description;
  if (typeof description !== "string" || description.trim().length === 0) {
    throw new Error(`${source}: a skill description is required in SKILL.md frontmatter`);
  }
  if (description.length > 2048) {
    throw new Error(`${source}: description must be <= 2048 chars`);
  }
  const bytes = bundleSkillFiles({ "SKILL.md": content });
  return {
    name,
    description,
    contentHash: await hashBytes(bytes),
    bytes
  };
}

export async function buildCliTool(args: {
  readonly name: string;
  readonly description: string;
  readonly entry: string;
  readonly content: string;
}): Promise<CliToolDraft> {
  const inputSchema: ToolInputSchema = { type: "object", properties: {}, additionalProperties: true };
  const manifest = normalizeToolManifest("Tool.fromFiles", {
    name: args.name,
    description: args.description,
    input_schema: inputSchema,
    entry: args.entry
  }, { [args.entry]: args.content });
  const bytes = bundleToolFiles({ [args.entry]: args.content }, manifest);
  return {
    ref: {
      kind: "asset",
      contentHash: await hashBytes(bytes),
      ...manifest
    },
    bytes
  };
}

export async function buildCliAgentsMd(content: string, name: string): Promise<CliAgentsMdDraft> {
  if (typeof content !== "string" || content.length === 0) {
    throw new Error("AgentsMd.fromContent: content must be a non-empty string");
  }
  if (!/^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$/.test(name)) {
    throw new Error("AgentsMd.fromContent: name must be a lowercase workspace slug");
  }
  const bytes = zipSync({ "AGENTS.md": [TEXT.encode(content), { mtime: ZIP_EPOCH }] }, { level: 6 });
  return { name, contentHash: await hashBytes(bytes), bytes };
}

export async function buildCliFile(args: { readonly name: string; readonly content: string }): Promise<CliFileDraft> {
  const filename = sanitiseFilename(args.name);
  if (filename === undefined) {
    throw new Error(`File.fromBytes: name ${JSON.stringify(args.name)} is not a valid filename`);
  }
  const bytes = TEXT.encode(args.content);
  if (bytes.byteLength === 0) {
    throw new Error("File.fromBytes: bytes must be a non-empty Uint8Array");
  }
  const zip = zipSync({ [filename]: [bytes, { mtime: ZIP_EPOCH }] }, { level: 6 });
  return {
    name: slugFromFilename(filename),
    contentHash: await hashBytes(zip),
    mountPath: DEFAULT_FILE_MOUNT_PATH,
    bytes: zip
  };
}

export function toCliSessionEnvironment(env: PlatformEnvironment | undefined): CliSessionEnvironmentOptions | undefined {
  if (!env) return undefined;
  const out: CliSessionEnvironmentOptions = {
    ...(env.networking ? { networking: env.networking } : {}),
    ...(env.packages ? { packages: env.packages } : {}),
    ...(env.envVars ? { variables: env.envVars } : {})
  };
  return Object.keys(out).length === 0 ? undefined : out;
}

async function buildSessionCreateRequest(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  options: CliRunSubmitOptions
): Promise<SessionCreateRequest> {
  const input = normaliseSessionInput(options.message);
  const provider = resolveModelProvider(options.model, options.provider);
  validateApiKeys(options.apiKeys, provider);

  try {
    parseRunTimeout(options.overrides?.timeout);
    if (options.webhook !== undefined) parseRunWebhook(options.webhook);
  } catch (err) {
    throw new Error(err instanceof Error ? err.message : String(err));
  }

  const limitsInput: { maxSpendUsd?: number; maxTurns?: number } = {};
  if (options.overrides?.maxSpendUsd !== undefined) limitsInput.maxSpendUsd = options.overrides.maxSpendUsd;
  if (options.overrides?.maxTurns !== undefined) limitsInput.maxTurns = options.overrides.maxTurns;
  const limits: RunLimits | undefined = parseRunLimits(Object.keys(limitsInput).length > 0 ? limitsInput : undefined);

  const [preparedTools, preparedSkills, preparedAgentsMd, preparedFiles] = await Promise.all([
    prepareTools(http, fetchImpl, options.tools ?? []),
    prepareSkills(http, fetchImpl, options.skills ?? []),
    prepareAgentsMd(http, fetchImpl, options.agentsMd ?? []),
    prepareFiles(http, fetchImpl, options.files ?? [])
  ]);
  const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(options.mcpServers ?? []);
  const environment = sessionEnvironmentForWire(options.environment);

  const submission: SessionCreateRequest["submission"] = {
    model: options.model,
    ...(options.system ? { system: options.system } : {}),
    tools: [...preparedTools.builtinNames, ...preparedTools.refs] as unknown as readonly ToolRef[],
    ...(preparedSkills.length > 0 ? { skills: preparedSkills } : {}),
    agentsMd: preparedAgentsMd,
    files: preparedFiles,
    mcpServers: submissionMcpServers,
    ...(environment ? { environment: environment as NonNullable<PlatformSubmission["environment"]> } : {}),
    ...(options.metadata ? { metadata: options.metadata } : {})
  };

  const secrets: PlatformInlineSecrets = {
    ...(options.apiKeys ? { apiKeys: options.apiKeys } : {}),
    ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {})
  };

  return {
    provider,
    submission,
    input,
    ...(options.runtime ? { runtimeSize: options.runtime } : {}),
    ...(options.overrides?.timeout ? { timeout: options.overrides.timeout } : {}),
    ...(limits ? { limits } : {}),
    retention: { idleTtl: options.overrides?.idleTtl ?? "3m" },
    ...(options.webhook ? { webhook: options.webhook } : {}),
    secrets
  };
}

function normaliseSessionInput(input: string | readonly string[]): string | readonly string[] {
  if (typeof input === "string") {
    if (!input) throw new Error("Aex.submit: message must be a non-empty string");
    return input;
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new Error("Aex.submit: message must be a non-empty string or string array");
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new Error("Aex.submit: message segments must be non-empty strings");
    }
  }
  return [...input];
}

function validateApiKeys(apiKeys: Partial<Record<RunProvider, string>> | undefined, provider: RunProvider): void {
  const key = apiKeys?.[provider];
  if (typeof key !== "string" || key.length === 0) {
    throw new Error(
      `Aex.submit: a provider API key is required for provider ${provider}; pass apiKeys.${provider}`
    );
  }
}

async function prepareTools(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  tools: readonly (CliToolDraft | BuiltinToolName)[]
): Promise<{ readonly refs: readonly ToolRef[]; readonly builtinNames: readonly BuiltinToolName[] }> {
  const prepared = await mapWithConcurrency(tools, UPLOAD_CONCURRENCY, async (entry, i) => {
    if (typeof entry === "string") {
      if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)) {
        throw new Error(`aex: tools[${i}] (${JSON.stringify(entry)}) is not a builtin tool name`);
      }
      return { kind: "builtin" as const, name: entry };
    }
    const uploaded = await uploadAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.ref.contentHash,
      contentType: "application/zip"
    });
    const { contentHash: _contentHash, ...ref } = entry.ref;
    void _contentHash;
    return { kind: "ref" as const, ref: { ...ref, assetId: uploaded.assetId } };
  });
  const refs: ToolRef[] = [];
  const seenBuiltins = new Set<BuiltinToolName>();
  const builtinNames: BuiltinToolName[] = [];
  for (const item of prepared) {
    if (item.kind === "builtin") {
      if (!seenBuiltins.has(item.name)) {
        seenBuiltins.add(item.name);
        builtinNames.push(item.name);
      }
    } else {
      refs.push(item.ref);
    }
  }
  return { refs, builtinNames };
}

async function prepareSkills(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  skills: readonly CliSkillDraft[]
): Promise<readonly { readonly kind: "skill"; readonly name: string }[]> {
  if (skills.length > SKILLS_MAX) {
    throw new Error(`aex: skills exceeds the ${SKILLS_MAX}-skill limit (got ${skills.length})`);
  }
  const seen = new Set<string>();
  for (const skill of skills) {
    if (seen.has(skill.name)) throw new Error(`aex: skills duplicate name: ${skill.name}`);
    seen.add(skill.name);
  }
  return mapWithConcurrency(skills, UPLOAD_CONCURRENCY, async (skill) => {
    await uploadAsset(http, fetchImpl, { bytes: skill.bytes, hash: skill.contentHash, contentType: "application/zip" });
    await operations.upsertSkill(http, {
      name: skill.name,
      contentHash: skill.contentHash,
      description: skill.description,
      sizeBytes: skill.bytes.byteLength
    });
    return { kind: "skill", name: skill.name };
  });
}

async function prepareAgentsMd(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  agentsMds: readonly CliAgentsMdDraft[]
): Promise<readonly AgentsMdRef[]> {
  return mapWithConcurrency(agentsMds, UPLOAD_CONCURRENCY, async (entry) => {
    const uploaded = await uploadAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.contentHash,
      contentType: "application/zip"
    });
    return { kind: "asset", assetId: uploaded.assetId, name: entry.name };
  });
}

async function prepareFiles(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  files: readonly CliFileDraft[]
): Promise<readonly FileRef[]> {
  return mapWithConcurrency(files, UPLOAD_CONCURRENCY, async (entry) => {
    const uploaded = await uploadAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.contentHash,
      contentType: "application/zip"
    });
    return { kind: "asset", assetId: uploaded.assetId, name: entry.name, mountPath: entry.mountPath };
  });
}

function mergeMcpServers(inputs: readonly CliMcpServer[]): {
  readonly submissionMcpServers: readonly McpServerRef[];
  readonly mergedMcpSecrets: readonly PlatformMcpServerSecret[];
} {
  const submissionMcpServers: McpServerRef[] = [];
  const mergedMcpSecrets: PlatformMcpServerSecret[] = [];
  for (const entry of inputs) {
    submissionMcpServers.push({ name: entry.name, url: entry.url });
    if (entry.headers && Object.keys(entry.headers).length > 0) {
      mergedMcpSecrets.push({ name: entry.name, url: entry.url, headers: { ...entry.headers } });
    }
  }
  return { submissionMcpServers, mergedMcpSecrets };
}

function sessionEnvironmentForWire(environment: CliSessionEnvironmentOptions | undefined): PlatformEnvironmentInput | undefined {
  if (environment === undefined) return undefined;
  const { variables, ...rest } = environment;
  const out: PlatformEnvironmentInput = {
    ...rest,
    ...(variables !== undefined ? { envVars: variables } : {})
  };
  return Object.keys(out).length === 0 ? undefined : out;
}

async function uploadAsset(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  args: { readonly bytes: Uint8Array; readonly hash: string; readonly contentType?: string }
): Promise<{ readonly assetId: string; readonly contentHash: string; readonly sizeBytes: number; readonly exists: boolean }> {
  const expected = args.hash.startsWith("sha256:") ? args.hash.slice("sha256:".length) : args.hash;
  const actual = await sha256Hex(args.bytes);
  if (actual !== expected) {
    throw new Error(`uploadAsset: client-side hash mismatch: computed sha256:${actual} but caller declared ${args.hash}`);
  }
  const contentHash = `sha256:${actual}`;
  const presign = await http.request<{
    readonly exists: boolean;
    readonly assetId?: string;
    readonly contentHash?: string;
    readonly sizeBytes?: number;
    readonly uploadUrl?: string;
    readonly requiredHeaders?: Record<string, string>;
  }>("/assets/presign", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ hash: contentHash, sizeBytes: args.bytes.byteLength })
  });
  if (presign.exists) {
    const storedHash = presign.contentHash ?? contentHash;
    return {
      assetId: presign.assetId ?? assetIdFromContentHash(storedHash),
      contentHash: storedHash,
      sizeBytes: presign.sizeBytes ?? args.bytes.byteLength,
      exists: true
    };
  }
  if (!presign.uploadUrl) {
    throw new Error("uploadAsset: presign returned no uploadUrl and exists:false");
  }
  const doFetch = fetchImpl ?? (globalThis.fetch as FetchLike);
  const put = await doFetch(presign.uploadUrl, {
    method: "PUT",
    headers: {
      "content-type": args.contentType ?? "application/zip",
      ...(presign.requiredHeaders ?? {})
    },
    body: args.bytes
  });
  if (!put.ok) {
    throw new Error(`uploadAsset: direct upload PUT failed with status ${put.status}`);
  }
  const fin = await http.request<{
    readonly assetId?: string;
    readonly contentHash?: string;
    readonly sizeBytes?: number;
  }>("/assets/finalize", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ hash: contentHash, sizeBytes: args.bytes.byteLength })
  });
  const storedHash = fin.contentHash ?? presign.contentHash ?? contentHash;
  return {
    assetId: fin.assetId ?? presign.assetId ?? assetIdFromContentHash(storedHash),
    contentHash: storedHash,
    sizeBytes: fin.sizeBytes ?? args.bytes.byteLength,
    exists: false
  };
}

function bundleSkillFiles(files: Readonly<Record<string, string | Uint8Array>>): Uint8Array {
  const collected = collectBundleFiles("Skill bundle", files, true);
  return zipCollected(collected);
}

function bundleToolFiles(
  files: Readonly<Record<string, string | Uint8Array>>,
  manifest: ToolBundleManifest
): Uint8Array {
  const collected = collectBundleFiles("Tool bundle", files, false);
  const entryPath = validateSkillBundleEntry({ path: manifest.entry, size: 0 }).path;
  if (!collected.has(entryPath)) {
    throw new Error(`Tool bundle entry "${entryPath}" must exist in files`);
  }
  if (collected.has("tool.json")) {
    throw new Error('Tool bundle files must not include reserved "tool.json"; pass manifest fields instead');
  }
  collected.set("tool.json", TEXT.encode(`${JSON.stringify(manifest, null, 2)}\n`));
  return zipCollected(collected);
}

function collectBundleFiles(
  kind: string,
  files: Readonly<Record<string, string | Uint8Array>>,
  requireSkillMd: boolean
): Map<string, Uint8Array> {
  const entries = Object.entries(files);
  if (entries.length === 0) throw new Error(`${kind} files map cannot be empty`);
  if (entries.length > SKILL_BUNDLE_LIMITS.maxFiles) {
    throw new Error(`${kind} exceeds ${SKILL_BUNDLE_LIMITS.maxFiles} file limit (got ${entries.length})`);
  }
  const collected = new Map<string, Uint8Array>();
  let hasSkillMd = false;
  let total = 0;
  for (const [rawPath, contents] of entries) {
    const bytes = typeof contents === "string" ? TEXT.encode(contents) : contents;
    if (!(bytes instanceof Uint8Array)) throw new Error(`${kind} file "${rawPath}" must be a string or Uint8Array`);
    const entry = validateSkillBundleEntry({ path: rawPath, size: bytes.byteLength });
    if (entry.path === "SKILL.md") hasSkillMd = true;
    total += bytes.byteLength;
    if (total > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(`${kind} exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`);
    }
    if (collected.has(entry.path)) throw new Error(`${kind} contains duplicate path: ${entry.path}`);
    collected.set(entry.path, bytes);
  }
  if (requireSkillMd && !hasSkillMd) {
    throw new Error('Skill bundle must contain a "SKILL.md" file at the root.');
  }
  return collected;
}

function zipCollected(collected: Map<string, Uint8Array>): Uint8Array {
  const sorted = [...collected.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const zippable: Zippable = {};
  for (const [path, bytes] of sorted) {
    zippable[path] = [bytes, { mtime: ZIP_EPOCH }];
  }
  const zip = zipSync(zippable, { level: 6 });
  if (zip.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(`bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes (got ${zip.byteLength})`);
  }
  return zip;
}

interface ToolBundleManifest {
  readonly name: string;
  readonly description: string;
  readonly input_schema: ToolInputSchema;
  readonly entry: string;
}

function normalizeToolManifest(
  source: string,
  input: ToolBundleManifest,
  files: Readonly<Record<string, string | Uint8Array>>
): ToolBundleManifest {
  if (typeof input.name !== "string" || !TOOL_NAME_PATTERN.test(input.name)) {
    throw new Error(`${source}: name must match ${TOOL_NAME_PATTERN.source}`);
  }
  if (input.name.includes("__")) {
    throw new Error(`${source}: name must not contain "__"; that separator is reserved for MCP tools`);
  }
  if (typeof input.description !== "string" || input.description.trim().length === 0 || input.description.length > 2048) {
    throw new Error(`${source}: description must be non-empty and <= 2048 chars`);
  }
  const inputSchema = input.input_schema;
  if (!inputSchema || typeof inputSchema !== "object" || Array.isArray(inputSchema)) {
    throw new Error(`${source}: inputSchema must be a JSON Schema object`);
  }
  if ((inputSchema as { readonly type?: unknown }).type !== "object") {
    throw new Error(`${source}: inputSchema.type must be "object"`);
  }
  const entry = normaliseSkillBundlePath(input.entry);
  if (!/\.(?:js|mjs|cjs)$/i.test(entry.split("/").pop() ?? entry)) {
    throw new Error(`${source}: entry must be a JS module (.js/.mjs/.cjs)`);
  }
  if (!(entry in files) && !(input.entry in files)) {
    throw new Error(`${source}: entry ${JSON.stringify(input.entry)} is not present in files`);
  }
  return { ...input, entry };
}

function extractSkillFrontmatter(source: string, files: Readonly<Record<string, string | Uint8Array>>): { name?: string; description?: string } {
  const raw = files["SKILL.md"];
  if (raw === undefined) throw new Error(`${source}: the skill bundle must contain a SKILL.md at its root`);
  const text = typeof raw === "string" ? raw : new TextDecoder().decode(raw);
  return parseSkillFrontmatter(text);
}

function parseSkillFrontmatter(text: string): { name?: string; description?: string } {
  const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
  const match = /^---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(src);
  if (!match) return {};
  const out: { name?: string; description?: string } = {};
  for (const line of match[1]!.split(/\r?\n/)) {
    const kv = /^([A-Za-z0-9_-]+)[ \t]*:[ \t]*(.*)$/.exec(line);
    if (!kv) continue;
    const key = kv[1]!.toLowerCase();
    if (key !== "name" && key !== "description") continue;
    let value = kv[2]!.trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    if (value.length > 0) out[key] = value;
  }
  return out;
}

function deriveSkillName(
  source: string,
  frontmatterName: string | undefined,
  explicitName: string | undefined,
  dirBasename: string | undefined
): string {
  let name: string | undefined = explicitName ?? frontmatterName;
  if (name === undefined && dirBasename !== undefined) {
    const slug = slugifyName(dirBasename);
    if (slug.length > 0) name = slug;
  }
  if (typeof name !== "string" || name.length === 0) {
    throw new Error(`${source}: a skill name is required`);
  }
  if (!SKILL_NAME_PATTERN.test(name)) {
    throw new Error(`${source}: name ${JSON.stringify(name)} must match ${SKILL_NAME_PATTERN.source}`);
  }
  if (name.includes("__")) {
    throw new Error(`${source}: name must not contain "__"; that separator is reserved for MCP tools`);
  }
  if (SKILL_RESERVED_NAMES.has(name)) {
    throw new Error(`${source}: name ${JSON.stringify(name)} is reserved (${[...SKILL_RESERVED_NAMES].join(", ")})`);
  }
  return name;
}

function sanitiseFilename(name: string): string | undefined {
  if (typeof name !== "string" || name.length === 0 || name.length > 255) return undefined;
  if (name.includes("/") || name.includes("\\") || name.includes("\0")) return undefined;
  if (name === "." || name === "..") return undefined;
  return name;
}

function slugFromFilename(filename: string): string {
  const stem = filename.includes(".") ? filename.slice(0, filename.lastIndexOf(".")) : filename;
  const slug = slugifyName(stem);
  return slug.length > 0 ? slug : "file";
}

function slugifyName(input: string): string {
  return input.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

async function hashBytes(bytes: Uint8Array): Promise<string> {
  return `sha256:${await sha256Hex(bytes)}`;
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error("sha256: globalThis.crypto.subtle is not available");
  }
  const view = new Uint8Array(bytes.byteLength);
  view.set(bytes);
  const digest = await subtle.digest("SHA-256", view.buffer);
  return bufferToHex(digest);
}

function bufferToHex(buffer: ArrayBuffer): string {
  const view = new Uint8Array(buffer);
  let out = "";
  for (const byte of view) {
    out += byte.toString(16).padStart(2, "0");
  }
  return out;
}

function assetIdFromContentHash(contentHash: string): string {
  const hex = contentHash.startsWith("sha256:") ? contentHash.slice("sha256:".length) : contentHash;
  return `asset_${hex}`;
}

async function mapWithConcurrency<T, R>(
  items: readonly T[],
  limit: number,
  fn: (item: T, index: number) => Promise<R>
): Promise<R[]> {
  const out = new Array<R>(items.length);
  let next = 0;
  const lanes = Array.from({ length: Math.min(Math.max(1, limit), items.length) }, async () => {
    for (let i = next++; i < items.length; i = next++) {
      out[i] = await fn(items[i]!, i);
    }
  });
  await Promise.all(lanes);
  return out;
}
