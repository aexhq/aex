import type {
  BuiltinTool as ContractBuiltinTool,
  ToolsConfig,
} from "@aexhq/contracts/session";

export type BuiltinToolName = ContractBuiltinTool;

/** One built-in Aex capability selected by an imported tool helper. */
export interface BuiltinTool {
  readonly kind: "aex.builtin";
  readonly name: BuiltinToolName;
}

export type Tool = BuiltinTool;

const BUILTIN_NAMES: ReadonlySet<string> = new Set<BuiltinToolName>([
  "bash",
  "read",
  "write",
  "edit",
  "glob",
  "grep",
  "ls",
  "task",
  "todo",
  "web_search",
  "web_fetch",
]);

/** Compile public tool values into the session API's sealed wire configuration. */
export function compileTools(selections: readonly Tool[] | undefined): ToolsConfig {
  const builtin: BuiltinToolName[] = [];
  const seen = new Set<BuiltinToolName>();

  const append = (tool: BuiltinTool): void => {
    if (tool?.kind !== "aex.builtin" || !BUILTIN_NAMES.has(tool.name)) {
      throw new TypeError("Invalid Aex tool value; import tools from @aexhq/tools");
    }
    if (seen.has(tool.name)) {
      throw new TypeError(`Aex tool ${tool.name} was selected more than once`);
    }
    seen.add(tool.name);
    builtin.push(tool.name);
  };

  for (const selection of selections ?? []) append(selection);

  return { builtin };
}
