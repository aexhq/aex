import {
  BUILTIN_TOOL_NAMES,
  DEFAULT_FILE_MOUNT_PATH,
  assertWorkspaceInstructionResourceName,
  parseModelSlug,
  parseSessionLimits,
  parseSessionTimeout,
  parseSessionWebhook,
  type BuiltinToolName,
  type FetchLike,
  type HttpClient,
  type JsonValue,
  type McpServerRef,
  type PlatformEnvironment,
  type PlatformEnvironmentInput,
  type PlatformInlineSecrets,
  type PlatformMcpServerSecret,
  type PlatformSubmission,
  type SessionLimits,
  type ModelName,
  type SessionRuntime,
  type SessionMessageAccepted,
  type SessionCreateRequest,
  type ToolInputSchema,
  type ToolRef,
  type WorkspaceFileRef,
  type WorkspaceInstructionRef,
  type WorkspaceSkillRef,
  type WorkspaceToolRef
} from "@aexhq/contracts";
import {
  assertArchiveExpandedSize,
  bundleSingleFile,
  bundleSkillFiles,
  bundleToolFiles,
  deriveSkillName,
  extractSkillFrontmatter,
  hashSkillBundle,
  normalizeToolManifest,
  operations,
  slugifyAssetName,
  uploadAsset as uploadHostAsset
} from "@aexhq/contracts/internal";
import { StartValidationError, startValidationError, type StartFlag } from "./start-validation.js";

const TEXT = new TextEncoder();
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

export interface CliInstructionsDraft {
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

export interface CliSessionSubmitOptions {
  readonly message: string | readonly string[];
  readonly model: ModelName;
  readonly system?: string;
  readonly tools?: readonly (CliToolDraft | BuiltinToolName)[];
  readonly skills?: readonly CliSkillDraft[];
  readonly instructions?: readonly CliInstructionsDraft[];
  readonly files?: readonly CliFileDraft[];
  readonly mcpServers?: readonly CliMcpServer[];
  readonly metadata?: Readonly<Record<string, JsonValue>>;
  readonly environment?: CliSessionEnvironmentOptions;
  readonly runtime?: SessionRuntime;
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
  options: CliSessionSubmitOptions
): Promise<SessionMessageAccepted> {
  const request = await buildSessionCreateRequest(http, fetchImpl, options);
  const input = normaliseSessionInput(options.message);
  const submitted = await operations.createSessionWithMessage(
    http,
    request,
    input,
    { idempotencyKey: operations.resolveIdempotencyKey(options.idempotencyKey) }
  );
  return submitted;
}

export async function buildCliSkill(content: string, source: string): Promise<CliSkillDraft> {
  const front = extractSkillFrontmatter(source, { "SKILL.md": content });
  const name = deriveSkillName(source, front.name, undefined, undefined, "cli");
  const description = front.description;
  if (typeof description !== "string" || description.trim().length === 0) {
    throw new Error(`${source}: a skill description is required in SKILL.md frontmatter`);
  }
  if (description.length > 2048) {
    throw new Error(`${source}: description must be <= 2048 chars`);
  }
  const bytes = bundleSkillFiles({ "SKILL.md": content }, undefined, "cli").zip;
  return {
    name,
    description,
    contentHash: await hashSkillBundle(bytes, "cli"),
    bytes
  };
}

export async function buildCliTool(args: {
  readonly name: string;
  readonly description: string;
  readonly entry: string;
  readonly content: string;
}, source = "Tool.fromFiles"): Promise<CliToolDraft> {
  const inputSchema: ToolInputSchema = { type: "object", properties: {}, additionalProperties: true };
  const manifest = normalizeToolManifest(source, {
    name: args.name,
    description: args.description,
    input_schema: inputSchema,
    entry: args.entry
  }, { [args.entry]: args.content }, "cli");
  const bytes = bundleToolFiles({ [args.entry]: args.content }, manifest, undefined, "cli").zip;
  return {
    ref: {
      kind: "asset",
      contentHash: await hashSkillBundle(bytes, "cli"),
      ...manifest
    },
    bytes
  };
}

export async function buildCliInstructions(
  content: string,
  name: string,
  source = "Instructions.fromContent"
): Promise<CliInstructionsDraft> {
  if (typeof content !== "string" || content.length === 0) {
    throw new Error(`${source}: content must be a non-empty string`);
  }
  assertWorkspaceInstructionResourceName(name, `${source}: name`);
  const bytes = bundleSingleFile("AGENTS.md", TEXT.encode(content), source, false);
  return { name, contentHash: await hashSkillBundle(bytes, "cli"), bytes };
}

export async function buildCliFile(
  args: { readonly name: string; readonly bytes: Uint8Array },
  source = "File.fromBytes"
): Promise<CliFileDraft> {
  const filename = sanitiseFilename(args.name);
  if (filename === undefined) {
    throw new Error(`${source}: name ${JSON.stringify(args.name)} is not a valid filename`);
  }
  const bytes = args.bytes;
  if (!(bytes instanceof Uint8Array) || bytes.byteLength === 0) {
    throw new Error(`${source}: bytes must be a non-empty Uint8Array`);
  }
  assertArchiveExpandedSize(bytes.byteLength, source);
  const zip = bundleSingleFile(filename, bytes, source, false);
  return {
    name: slugFromFilename(filename),
    contentHash: await hashSkillBundle(zip, "cli"),
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
  options: CliSessionSubmitOptions
): Promise<SessionCreateRequest> {
  try {
    parseModelSlug(options.model, "--model");
  } catch (err) {
    throw startValidationError("--model", err);
  }

  try {
    parseSessionTimeout(options.overrides?.timeout);
  } catch (err) {
    throw startValidationError("--session-timeout", err);
  }
  try {
    if (options.webhook !== undefined) parseSessionWebhook(options.webhook);
  } catch (err) {
    throw startValidationError("--webhook", err);
  }

  const limitsInput: { maxSpendUsd?: number; maxTurns?: number } = {};
  if (options.overrides?.maxSpendUsd !== undefined) limitsInput.maxSpendUsd = options.overrides.maxSpendUsd;
  if (options.overrides?.maxTurns !== undefined) limitsInput.maxTurns = options.overrides.maxTurns;
  const limits: SessionLimits | undefined = parseSessionLimits(Object.keys(limitsInput).length > 0 ? limitsInput : undefined);

  const preparedTools = await prepareTools(http, fetchImpl, options.tools ?? []);
  const skills = await prepareSkills(http, fetchImpl, options.skills ?? []);
  const instructions = await prepareInstructions(http, fetchImpl, options.instructions ?? []);
  const files = await prepareFiles(http, fetchImpl, options.files ?? []);
  const { submissionMcpServers, mergedMcpSecrets } = mergeMcpServers(options.mcpServers ?? []);
  const environment = sessionEnvironmentForWire(options.environment);

  const submission: SessionCreateRequest["submission"] = {
    model: options.model,
    ...(options.system ? { system: options.system } : {}),
    assets: { files, skills, tools: preparedTools.refs, instructions },
    builtinTools: preparedTools.builtinNames.length > 0 ? preparedTools.builtinNames : "default",
    mcpServers: submissionMcpServers,
    ...(environment ? { environment: environment as NonNullable<PlatformSubmission["environment"]> } : {}),
    ...(options.metadata ? { metadata: options.metadata } : {})
  };

  const secrets: PlatformInlineSecrets = {
    ...(mergedMcpSecrets.length > 0 ? { mcpServers: mergedMcpSecrets } : {})
  };

  return {
    submission,
    ...(options.runtime?.size ? { runtimeSize: options.runtime.size } : {}),
    ...(options.runtime?.kind ? { runtimeKind: options.runtime.kind } : {}),
    ...(options.overrides?.timeout ? { timeout: options.overrides.timeout } : {}),
    ...(limits ? { limits } : {}),
    retention: { idleTtl: options.overrides?.idleTtl ?? "3m" },
    ...(options.webhook ? { webhook: options.webhook } : {}),
    secrets
  };
}

function normaliseSessionInput(input: string | readonly string[]): string | readonly string[] {
  if (typeof input === "string") {
    if (!input) throw new StartValidationError("--prompt", "message must be a non-empty string");
    return input;
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw new StartValidationError("--prompt", "message must be a non-empty string or string array");
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw new StartValidationError("--prompt", "message segments must be non-empty strings");
    }
  }
  return [...input];
}

async function prepareTools(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  tools: readonly (CliToolDraft | BuiltinToolName)[]
): Promise<{ readonly refs: readonly WorkspaceToolRef[]; readonly builtinNames: readonly BuiltinToolName[] }> {
  const prepared = await mapWithConcurrency(tools, UPLOAD_CONCURRENCY, async (entry, i) => {
    if (typeof entry === "string") {
      if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)) {
        throw new StartValidationError(
          "--tool",
          `tools[${i}] (${JSON.stringify(entry)}) is not a builtin tool name`
        );
      }
      return { kind: "builtin" as const, name: entry };
    }
    const uploaded = await stageAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.ref.contentHash,
      contentType: "application/zip"
    });
    const published = await operations.publishWorkspaceTool(http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: "application/zip",
      name: entry.ref.name,
      description: entry.ref.description,
      input_schema: entry.ref.input_schema,
      entry: entry.ref.entry
    });
    return { kind: "ref" as const, ref: published };
  });
  const refs: WorkspaceToolRef[] = [];
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
): Promise<readonly WorkspaceSkillRef[]> {
  const seen = new Set<string>();
  for (const skill of skills) {
    if (seen.has(skill.name)) throw new StartValidationError("--skill", `skills duplicate name: ${skill.name}`);
    seen.add(skill.name);
  }
  return mapWithConcurrency(skills, UPLOAD_CONCURRENCY, async (skill) => {
    const uploaded = await stageAsset(http, fetchImpl, {
      bytes: skill.bytes,
      hash: skill.contentHash,
      contentType: "application/zip"
    });
    return operations.publishWorkspaceSkill(http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: "application/zip",
      name: skill.name,
      description: skill.description
    });
  });
}

async function prepareInstructions(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  instructions: readonly CliInstructionsDraft[]
): Promise<readonly WorkspaceInstructionRef[]> {
  return mapWithConcurrency(instructions, UPLOAD_CONCURRENCY, async (entry) => {
    const uploaded = await stageAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.contentHash,
      contentType: "application/zip"
    });
    return operations.publishWorkspaceInstruction(http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: "application/zip",
      name: entry.name
    });
  });
}

async function prepareFiles(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  files: readonly CliFileDraft[]
): Promise<readonly WorkspaceFileRef[]> {
  return mapWithConcurrency(files, UPLOAD_CONCURRENCY, async (entry) => {
    const uploaded = await stageAsset(http, fetchImpl, {
      bytes: entry.bytes,
      hash: entry.contentHash,
      contentType: "application/zip"
    });
    return operations.publishWorkspaceFile(http, {
      assetId: uploaded.assetId,
      contentHash: uploaded.contentHash,
      sizeBytes: uploaded.sizeBytes,
      contentType: "application/zip",
      name: entry.name,
      mountPath: entry.mountPath
    });
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

async function stageAsset(
  http: HttpClient,
  fetchImpl: FetchLike | undefined,
  args: { readonly bytes: Uint8Array; readonly hash: string; readonly contentType?: string }
): Promise<{ readonly assetId: string; readonly contentHash: string; readonly sizeBytes: number; readonly exists: boolean }> {
  return uploadHostAsset({
    http,
    bytes: args.bytes,
    hash: args.hash,
    ...(args.contentType !== undefined ? { contentType: args.contentType } : {}),
    ...(fetchImpl !== undefined ? { fetch: fetchImpl } : {})
  });
}

function sanitiseFilename(name: string): string | undefined {
  if (typeof name !== "string" || name.length === 0 || name.length > 255) return undefined;
  if (name.includes("/") || name.includes("\\") || name.includes("\0")) return undefined;
  if (name === "." || name === "..") return undefined;
  return name;
}

function slugFromFilename(filename: string): string {
  const stem = filename.includes(".") ? filename.slice(0, filename.lastIndexOf(".")) : filename;
  const slug = slugifyAssetName(stem);
  return slug.length > 0 ? slug : "file";
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
