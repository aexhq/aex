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

/** A stable, ordered group of built-in tools. */
export interface Toolset {
  readonly kind: "aex.toolset";
  readonly tools: readonly BuiltinTool[];
}

export type Tool = BuiltinTool;
export type ToolSelection = Tool | Toolset;

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
export function compileTools(selections: readonly ToolSelection[] | undefined): ToolsConfig {
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

  for (const selection of selections ?? []) {
    if (selection?.kind === "aex.builtin") {
      append(selection);
    } else if (selection?.kind === "aex.toolset" && Array.isArray(selection.tools)) {
      for (const tool of selection.tools) append(tool);
    } else {
      throw new TypeError("Invalid Aex tool value; import tools from @aexhq/tools");
    }
  }

  return { builtin };
}
