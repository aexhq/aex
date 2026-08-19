import type { BuiltinToolName, Tool, Toolset } from "@aexhq/sdk";

function builtin(name: BuiltinToolName): Tool {
  return Object.freeze({ kind: "aex.builtin", name });
}

function group(tools: readonly Tool[]): Toolset {
  return Object.freeze({ kind: "aex.toolset", tools: Object.freeze([...tools]) });
}

export const bash = (): Tool => builtin("bash");
export const read = (): Tool => builtin("read");
export const write = (): Tool => builtin("write");
export const edit = (): Tool => builtin("edit");
export const glob = (): Tool => builtin("glob");
export const grep = (): Tool => builtin("grep");
export const ls = (): Tool => builtin("ls");
export const todo = (): Tool => builtin("todo");
export const webSearch = (): Tool => builtin("web_search");
export const webFetch = (): Tool => builtin("web_fetch");

/**
 * Let the agent delegate self-contained work to bounded, in-process child agents.
 * The stable session wire calls this primitive `task`; applications select it by intent.
 */
export const subagents = (): Tool => builtin("task");

/** All seven tools backed by the session's managed computer. */
export const computer = (): Toolset =>
  group([bash(), read(), write(), edit(), glob(), grep(), ls()]);
