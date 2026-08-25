import type { ComponentExtension } from "@aexhq/brain";
import type { ToolDefinition } from "@aexhq/brain/session";
import { callback as callbackComponent } from "@aexhq/env-app";
import * as z from "zod";

import { jcsSha256 } from "./json.js";

export type JsonSchema = Record<string, unknown>;

export interface ToolContext {
  readonly signal: AbortSignal;
  readonly operationId: string;
  readonly sessionId: string;
  readonly deadlineMs: number;
}

export type ToolHandler<Input extends z.ZodType, Output = unknown> = (
  input: z.output<Input>,
  context: ToolContext,
) => Output | Promise<Output>;

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
  readonly handler: ToolHandler<Input, Output>;
  describe(description: string): Tool<Input, Output>;
  named(name: string): Tool<Input, Output>;
  returns<Schema extends z.ZodType>(schema: Schema): Tool<Input, z.output<Schema>>;
}

interface Draft<Input extends z.ZodType, Output> {
  readonly name?: string;
  readonly description?: string;
  readonly input: Input;
  readonly output?: z.ZodType;
  readonly handler: ToolHandler<Input, Output>;
}

const EMPTY_INPUT = z.object({});
const TOOL_NAME = /^[A-Za-z_][A-Za-z0-9_-]{0,63}$/u;

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
    });
  }
  if (typeof maybeHandler !== "function") throw new TypeError("tool(schema, handler) requires a function");
  return makeTool({
    ...(maybeHandler.name === "" ? {} : { name: maybeHandler.name }),
    input: inputOrHandler,
    handler: maybeHandler,
  });
}

function makeTool<Input extends z.ZodType, Output>(draft: Draft<Input, Output>): Tool<Input, Output> {
  return Object.freeze({
    kind: "aex.tool" as const,
    ...(draft.name === undefined ? {} : { name: draft.name }),
    ...(draft.description === undefined ? {} : { description: draft.description }),
    input: draft.input,
    ...(draft.output === undefined ? {} : { output: draft.output }),
    handler: draft.handler,
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
  });
}

export interface ClientRegistration {
  readonly registration: string;
  readonly name: string;
  readonly contractDigest: string;
  readonly input: z.ZodType;
  readonly output?: z.ZodType;
  readonly handler: ToolHandler<z.ZodType>;
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
