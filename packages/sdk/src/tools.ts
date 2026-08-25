import {
  inspectEnvironment,
  isEnvironmentRef,
  type ComputerProfile,
  type EnvironmentProfile,
  type EnvironmentRef,
} from "@aexhq/environment";
import * as z from "zod";
import { callback as callbackComponent } from "@aexhq/env-app";
import type { ComponentExtension } from "@aexhq/brain";
import type { ToolDefinition } from "@aexhq/brain/session";

import { jcsSha256 } from "./json.js";

export type JsonSchema = Record<string, unknown>;

export interface ToolContext {
  readonly signal: AbortSignal;
  readonly operationId: string;
  readonly sessionId: string;
  readonly deadlineMs: number;
  readonly workspace?: string;
}

export type ToolHandler<Input extends z.ZodType, Output = unknown> = (
  input: z.output<Input>,
  context: ToolContext,
) => Output | Promise<Output>;

export type ToolSetupHandler = () => void | Promise<void>;

export interface ToolRequirements {
  readonly env?: readonly string[];
  readonly workspace?: boolean;
  readonly processes?: boolean;
  readonly network?: readonly { readonly host: string; readonly port: number }[];
  readonly streaming?: boolean;
  readonly recovery?: "retained" | "connection" | "replay_safe";
}

export interface PreparedArtifact {
  readonly digest: string;
  readonly target: ComputerProfile["platform"];
  readonly contentBase64: string;
  readonly bytes: number;
  readonly execute: string;
  readonly setup?: string;
}

export interface ToolContract {
  readonly name: string;
  readonly description?: string;
  readonly inputSchema: JsonSchema;
  readonly outputSchema?: JsonSchema;
  readonly contractDigest: string;
}

export interface Tool<Input extends z.ZodType = z.ZodType, Output = unknown> {
  readonly kind: "aex.tool";
  readonly name?: string;
  readonly description?: string;
  readonly input: Input;
  readonly output?: z.ZodType;
  readonly requirements: ToolRequirements;
  readonly handler: ToolHandler<Input, Output>;
  readonly setupHandler?: ToolSetupHandler;
  readonly artifact?: PreparedArtifact;
  describe(description: string): Tool<Input, Output>;
  named(name: string): Tool<Input, Output>;
  returns<Schema extends z.ZodType>(schema: Schema): Tool<Input, z.output<Schema>>;
  needs(requirements: ToolRequirements): Tool<Input, Output>;
  setup(handler: ToolSetupHandler): Tool<Input, Output>;
  bind<Environment extends EnvironmentRef>(environment: Environment): BoundTool<Environment, Input, Output>;
}

export interface BoundTool<
  Environment extends EnvironmentRef = EnvironmentRef,
  Input extends z.ZodType = z.ZodType,
  Output = unknown,
> {
  readonly kind: "aex.bound-tool";
  readonly tool: Tool<Input, Output>;
  readonly environment: Environment;
}

interface Draft<Input extends z.ZodType, Output> {
  readonly name?: string;
  readonly description?: string;
  readonly input: Input;
  readonly output?: z.ZodType;
  readonly requirements: ToolRequirements;
  readonly handler: ToolHandler<Input, Output>;
  readonly setupHandler?: ToolSetupHandler;
  readonly artifact?: PreparedArtifact;
}

const EMPTY_INPUT = z.object({});
const TOOL_NAME = /^[A-Za-z_][A-Za-z0-9_-]{0,63}$/u;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/u;
const SHA256 = /^[0-9a-f]{64}$/u;

export function tool<Output>(handler: ToolHandler<typeof EMPTY_INPUT, Output>): Tool<typeof EMPTY_INPUT, Output>;
export function tool<Input extends z.ZodType, Output>(
  input: Input,
  handler: ToolHandler<Input, Output>,
): Tool<Input, Output>;
export function tool<Input extends z.ZodType, Output>(
  inputOrHandler: Input | ToolHandler<typeof EMPTY_INPUT, Output>,
  maybeHandler?: ToolHandler<Input, Output>,
): Tool<Input | typeof EMPTY_INPUT, Output> {
  if (typeof inputOrHandler === "function") {
    return makeTool({
      ...(inputOrHandler.name === "" ? {} : { name: inputOrHandler.name }),
      input: EMPTY_INPUT,
      handler: inputOrHandler,
      requirements: Object.freeze({}),
    });
  }
  if (typeof maybeHandler !== "function") throw new TypeError("tool(schema, handler) requires a function");
  return makeTool({
    ...(maybeHandler.name === "" ? {} : { name: maybeHandler.name }),
    input: inputOrHandler,
    handler: maybeHandler,
    requirements: Object.freeze({}),
  });
}

function makeTool<Input extends z.ZodType, Output>(draft: Draft<Input, Output>): Tool<Input, Output> {
  return Object.freeze({
    kind: "aex.tool" as const,
    ...(draft.name === undefined ? {} : { name: draft.name }),
    ...(draft.description === undefined ? {} : { description: draft.description }),
    input: draft.input,
    ...(draft.output === undefined ? {} : { output: draft.output }),
    requirements: draft.requirements,
    handler: draft.handler,
    ...(draft.setupHandler === undefined ? {} : { setupHandler: draft.setupHandler }),
    ...(draft.artifact === undefined ? {} : { artifact: draft.artifact }),
    describe(description: string) {
      if (description.trim() === "" || description.length > 4096) {
        throw new TypeError("Tool description must contain 1 through 4096 characters");
      }
      return makeTool({ ...draft, description });
    },
    named(name: string) {
      assertToolName(name);
      return makeTool({ ...draft, name });
    },
    returns<Schema extends z.ZodType>(output: Schema) {
      schemaOf(output, `${draft.name ?? "Tool"} output`);
      return makeTool({ ...draft, output }) as unknown as Tool<Input, z.output<Schema>>;
    },
    needs(requirements: ToolRequirements) {
      return makeTool({ ...draft, requirements: normalizeRequirements(requirements) });
    },
    setup(handler: ToolSetupHandler) {
      if (typeof handler !== "function") throw new TypeError("Tool setup requires a function");
      return makeTool({ ...draft, setupHandler: handler });
    },
    bind<Environment extends EnvironmentRef>(environment: Environment) {
      if (!isEnvironmentRef(environment)) throw new TypeError("Tool binding requires an EnvironmentRef");
      return Object.freeze({ kind: "aex.bound-tool" as const, tool: makeTool(draft), environment });
    },
  });
}

export function withPreparedArtifact<Input extends z.ZodType, Output>(
  value: Tool<Input, Output>,
  artifact: PreparedArtifact,
): Tool<Input, Output> {
  assertTool(value);
  assertArtifact(artifact);
  return makeTool({
    ...(value.name === undefined ? {} : { name: value.name }),
    ...(value.description === undefined ? {} : { description: value.description }),
    input: value.input,
    ...(value.output === undefined ? {} : { output: value.output }),
    requirements: value.requirements,
    handler: value.handler,
    ...(value.setupHandler === undefined ? {} : { setupHandler: value.setupHandler }),
    artifact: Object.freeze({ ...artifact }),
  });
}

export type EnvironmentMap = Record<string, EnvironmentRef>;
export type EnvironmentValue<Environments extends EnvironmentMap> = Environments[keyof Environments];
export type ToolSelection<Environment extends EnvironmentRef = EnvironmentRef> = Tool | BoundTool<Environment>;

export interface ClientRegistration {
  readonly registration: string;
  readonly name: string;
  readonly contractDigest: string;
  readonly input: z.ZodType;
  readonly output?: z.ZodType;
  readonly handler: ToolHandler<z.ZodType>;
}

export interface CompiledTools {
  readonly items: readonly unknown[];
  readonly bundles: readonly unknown[];
  readonly clientRegistrations: readonly ClientRegistration[];
  readonly environments: Readonly<Record<string, unknown>>;
  readonly environmentNames: ReadonlyMap<EnvironmentRef, string>;
  readonly callbackClientId?: string;
}

export interface CompiledCallbacks {
  readonly components: readonly ComponentExtension<
    "tool",
    Readonly<Record<string, unknown>> & { readonly definition: ToolDefinition }
  >[];
  readonly registrations: readonly ClientRegistration[];
}

export async function compileCallbacks(selections: readonly Tool[]): Promise<CompiledCallbacks> {
  const components: ComponentExtension<
    "tool",
    Readonly<Record<string, unknown>> & { readonly definition: ToolDefinition }
  >[] = [];
  const registrations: ClientRegistration[] = [];
  const names = new Set<string>();
  for (const value of selections) {
    assertTool(value);
    const contract = await compileContract(value);
    if (names.has(contract.name)) {
      throw new TypeError(`Tool ${JSON.stringify(contract.name)} was selected twice`);
    }
    names.add(contract.name);
    if (value.artifact !== undefined) {
      throw new TypeError(
        `Tool ${JSON.stringify(contract.name)} is prepared for a hosted runtime and cannot execute as an application callback`,
      );
    }
    if (value.requirements.workspace === true || value.requirements.processes === true) {
      throw new TypeError(
        `Tool ${JSON.stringify(contract.name)} requires hosted workspace or process capabilities`,
      );
    }
    const definition = {
      name: contract.name,
      ...(contract.description === undefined ? {} : { description: contract.description }),
      input_schema: contract.inputSchema,
      output_schema: contract.outputSchema ?? {},
      contract_digest: contract.contractDigest,
    };
    const registration = `tool:${contract.contractDigest}`;
    components.push(callbackComponent(definition, registration));
    registrations.push(Object.freeze({
      registration,
      name: contract.name,
      contractDigest: contract.contractDigest,
      input: value.input,
      ...(value.output === undefined ? {} : { output: value.output }),
      handler: value.handler as ToolHandler<z.ZodType>,
    }));
  }
  return {
    components: Object.freeze(components),
    registrations: Object.freeze(registrations),
  };
}

export async function compileTools(
  selections: readonly ToolSelection[] | undefined,
  declared: EnvironmentMap | undefined,
  secrets: Readonly<Record<string, string>> = {},
): Promise<CompiledTools> {
  const environments = normalizeEnvironments(declared);
  const selected = [...(selections ?? [])];
  if (selected.length > 0 && environments.byName.size === 0) {
    throw new TypeError("Tools require at least one declared environment");
  }
  const items: unknown[] = [];
  const bundles = new Map<string, unknown>();
  const registrations: ClientRegistration[] = [];
  const names = new Set<string>();
  let callbackClientId: string | undefined;

  for (const selection of selected) {
    const value = selection.kind === "aex.bound-tool" ? selection.tool : selection;
    assertTool(value);
    const contract = await compileContract(value);
    if (names.has(contract.name)) throw new TypeError(`Tool ${JSON.stringify(contract.name)} was selected twice`);
    names.add(contract.name);
    const explicit = selection.kind === "aex.bound-tool" ? selection.environment : undefined;
    const environment = chooseEnvironment(contract.name, value, explicit, environments);
    const environmentName = environments.byRef.get(environment)!;
    const environmentProfile = inspectEnvironment(environment).serialized.profile;
    for (const key of value.requirements.env ?? []) {
      if (!(key in secrets)) throw new TypeError(`Tool ${JSON.stringify(contract.name)} requires missing secret ${JSON.stringify(key)}`);
    }

    const definition = {
      name: contract.name,
      ...(contract.description === undefined ? {} : { description: contract.description }),
      input_schema: contract.inputSchema,
      output_schema: contract.outputSchema ?? {},
      contract_digest: contract.contractDigest,
    };
    if (environmentProfile.kind === "callbacks") {
      const descriptor = inspectEnvironment(environment).serialized;
      const id = descriptor.configuration.id;
      if (typeof id !== "string" || id.trim() === "") {
        throw new TypeError(`Callback environment ${JSON.stringify(environmentName)} must serialize a non-empty id`);
      }
      if (callbackClientId !== undefined && callbackClientId !== id) {
        throw new TypeError("A session currently supports one callback process id");
      }
      callbackClientId = id;
      const registration = `tool:${contract.contractDigest}`;
      registrations.push(Object.freeze({
        registration,
        name: contract.name,
        contractDigest: contract.contractDigest,
        input: value.input,
        ...(value.output === undefined ? {} : { output: value.output }),
        handler: value.handler as ToolHandler<z.ZodType>,
      }));
      items.push({
        definition,
        executor: {
          kind: "environment",
          environment: environmentName,
          callback_registration: registration,
          requirements: wireRequirements(value.requirements),
        },
      });
      continue;
    }

    if (value.artifact === undefined) {
      throw new TypeError(`Tool ${JSON.stringify(contract.name)} has no prepared computer artifact`);
    }
    assertArtifact(value.artifact);
    bundles.set(value.artifact.digest, {
      checksum: value.artifact.digest,
      content_base64: value.artifact.contentBase64,
      bytes: value.artifact.bytes,
      media_type: "application/javascript+esm",
    });
    items.push({
      definition,
      executor: {
        kind: "environment",
        environment: environmentName,
        artifact_digest: value.artifact.digest,
        requirements: wireRequirements(value.requirements),
      },
    });
  }

  return {
    items: Object.freeze(items),
    bundles: Object.freeze([...bundles.values()]),
    clientRegistrations: Object.freeze(registrations),
    environments: environments.serialized,
    environmentNames: environments.byRef,
    ...(callbackClientId === undefined ? {} : { callbackClientId }),
  };
}

interface NormalizedEnvironments {
  readonly byName: ReadonlyMap<string, EnvironmentRef>;
  readonly byRef: ReadonlyMap<EnvironmentRef, string>;
  readonly serialized: Readonly<Record<string, unknown>>;
}

function normalizeEnvironments(declared: EnvironmentMap | undefined): NormalizedEnvironments {
  const byName = new Map<string, EnvironmentRef>();
  const byRef = new Map<EnvironmentRef, string>();
  const serialized: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
  for (const [name, environment] of Object.entries(declared ?? {})) {
    if (!/^[A-Za-z][A-Za-z0-9_-]{0,63}$/u.test(name)) {
      throw new TypeError(`Invalid environment name ${JSON.stringify(name)}`);
    }
    if (!isEnvironmentRef(environment)) throw new TypeError(`Environment ${JSON.stringify(name)} is not an EnvironmentRef`);
    const previous = byRef.get(environment);
    if (previous !== undefined) {
      throw new TypeError(`The same EnvironmentRef was declared as ${JSON.stringify(previous)} and ${JSON.stringify(name)}`);
    }
    byName.set(name, environment);
    byRef.set(environment, name);
    serialized[name] = inspectEnvironment(environment).serialized;
  }
  return { byName, byRef, serialized: Object.freeze(serialized) };
}

function chooseEnvironment(
  toolName: string,
  value: Tool,
  explicit: EnvironmentRef | undefined,
  environments: NormalizedEnvironments,
): EnvironmentRef {
  if (explicit !== undefined) {
    const name = environments.byRef.get(explicit);
    if (name === undefined) throw new TypeError(`Tool ${JSON.stringify(toolName)} is bound to an environment absent from environments`);
    const reasons = incompatibilities(value, inspectEnvironment(explicit).serialized.profile);
    if (reasons.length > 0) {
      throw new TypeError(`Tool ${JSON.stringify(toolName)} is bound to ${JSON.stringify(name)}, which ${reasons.join(" and ")}`);
    }
    return explicit;
  }
  const matches: EnvironmentRef[] = [];
  const failures: string[] = [];
  for (const [name, environment] of environments.byName) {
    const reasons = incompatibilities(value, inspectEnvironment(environment).serialized.profile);
    if (reasons.length === 0) matches.push(environment);
    else failures.push(`- ${JSON.stringify(name)} ${reasons.join(" and ")}`);
  }
  if (matches.length === 1) return matches[0]!;
  if (matches.length === 0) {
    throw new TypeError(`Tool ${JSON.stringify(toolName)} has no compatible environment:\n${failures.join("\n")}`);
  }
  const names = matches.map((environment) => JSON.stringify(environments.byRef.get(environment))).join(", ");
  throw new TypeError(`Tool ${JSON.stringify(toolName)} matches more than one environment: ${names}. Bind it explicitly.`);
}

function incompatibilities(
  value: Tool,
  profile: { kind: EnvironmentProfile["kind"]; platform?: string; network: string; recovery: string },
): string[] {
  const reasons: string[] = [];
  if (profile.kind === "computer") {
    if (value.artifact === undefined || profile.platform !== value.artifact.target) {
      reasons.push(`cannot launch target ${JSON.stringify(value.artifact?.target ?? "application-callback")}`);
    }
  } else if (profile.kind !== "callbacks") {
    reasons.push("does not run application callbacks");
  }
  const needs = value.requirements;
  if (needs.workspace === true && profile.kind !== "computer") reasons.push("is missing workspace");
  if (needs.processes === true && profile.kind !== "computer") reasons.push("is missing processes");
  if (needs.network !== undefined && needs.network.length > 0 && profile.network !== "allowlist") {
    reasons.push("cannot enforce network allowlists");
  }
  if (needs.recovery !== undefined && needs.recovery !== profile.recovery) {
    reasons.push(`provides ${profile.recovery} recovery instead of ${needs.recovery}`);
  }
  return reasons;
}

async function compileContract(value: Tool): Promise<ToolContract> {
  const name = value.name;
  if (name === undefined) throw new TypeError("Tool functions must be named; use .named(name) when build tooling removes the name");
  assertToolName(name);
  const inputSchema = schemaOf(value.input, `${name} input`);
  const outputSchema = value.output === undefined ? undefined : schemaOf(value.output, `${name} output`);
  const canonical = {
    name,
    ...(value.description === undefined ? {} : { description: value.description }),
    input_schema: inputSchema,
    ...(outputSchema === undefined ? {} : { output_schema: outputSchema }),
  };
  return {
    name,
    ...(value.description === undefined ? {} : { description: value.description }),
    inputSchema,
    ...(outputSchema === undefined ? {} : { outputSchema }),
    contractDigest: await jcsSha256(canonical),
  };
}

function wireRequirements(requirements: ToolRequirements): Record<string, unknown> {
  return {
    ...(requirements.env === undefined ? {} : { env: [...requirements.env] }),
    ...(requirements.workspace === undefined ? {} : { workspace: requirements.workspace }),
    ...(requirements.processes === undefined ? {} : { processes: requirements.processes }),
    ...(requirements.network === undefined
      ? {}
      : { network: requirements.network.map(({ host, port }) => ({ host, ports: [port], protocol: "tls" })) }),
    ...(requirements.streaming === undefined ? {} : { streaming: requirements.streaming }),
    ...(requirements.recovery === undefined ? {} : { recovery: requirements.recovery }),
  };
}

function normalizeRequirements(requirements: ToolRequirements): ToolRequirements {
  const env = [...(requirements.env ?? [])];
  if (new Set(env).size !== env.length) throw new TypeError("Tool environment requirements contain duplicates");
  for (const key of env) if (!ENV_NAME.test(key)) throw new TypeError(`Invalid required environment name ${JSON.stringify(key)}`);
  const network = [...(requirements.network ?? [])];
  if (network.length > 32) throw new TypeError("Tool network requirements exceed 32 destinations");
  for (const destination of network) {
    if (destination.host.trim() === "" || destination.host.length > 253) throw new TypeError("Invalid tool network host");
    if (!Number.isInteger(destination.port) || destination.port < 1 || destination.port > 65535) {
      throw new TypeError("Invalid tool network port");
    }
  }
  return Object.freeze({
    ...(env.length === 0 ? {} : { env: Object.freeze(env) }),
    ...(requirements.workspace === undefined ? {} : { workspace: requirements.workspace }),
    ...(requirements.processes === undefined ? {} : { processes: requirements.processes }),
    ...(network.length === 0 ? {} : { network: Object.freeze(network.map((item) => Object.freeze({ ...item }))) }),
    ...(requirements.streaming === undefined ? {} : { streaming: requirements.streaming }),
    ...(requirements.recovery === undefined ? {} : { recovery: requirements.recovery }),
  });
}

function schemaOf(schema: z.ZodType, label: string): JsonSchema {
  try {
    const value = z.toJSONSchema(schema, { target: "draft-2020-12", unrepresentable: "throw" }) as JsonSchema;
    if (Object.keys(value).length === 0) throw new TypeError(`${label} schema is empty`);
    return value;
  } catch (cause) {
    throw new TypeError(`${label} cannot be represented as JSON Schema`, { cause });
  }
}

function assertTool(value: Tool): void {
  if (value?.kind !== "aex.tool") throw new TypeError("Tool selection is invalid");
}

function assertToolName(name: string): void {
  if (!TOOL_NAME.test(name)) throw new TypeError(`Invalid Tool name ${JSON.stringify(name)}`);
}

function assertArtifact(artifact: PreparedArtifact): void {
  if (!SHA256.test(artifact.digest)) throw new TypeError("Prepared artifact digest must be lower-case SHA-256 hex");
  if (!Number.isSafeInteger(artifact.bytes) || artifact.bytes < 1) throw new TypeError("Prepared artifact bytes are invalid");
  if (!artifact.execute.startsWith("/")) throw new TypeError("Prepared artifact execute entrypoint must be absolute");
  if (artifact.setup !== undefined && !artifact.setup.startsWith("/")) {
    throw new TypeError("Prepared artifact setup entrypoint must be absolute");
  }
}
