import type { Tool } from "@aexhq/brain";
import { officialTool } from "@aexhq/brain/internal";
import {
  bash as portableBash,
  edit as portableEdit,
  glob as portableGlob,
  grep as portableGrep,
  ls as portableLs,
  read as portableRead,
  sandbox as portableSandbox,
  storage as portableStorage,
  subagents as portableSubagents,
  todo as portableTodo,
  write as portableWrite,
} from "@aexhq/brain-tools";
import { z } from "zod";

const selected = (tool: Tool): (() => Tool) => () => tool;

export const bash = selected(portableBash);
export const read = selected(portableRead);
export const write = selected(portableWrite);
export const edit = selected(portableEdit);
export const glob = selected(portableGlob);
export const grep = selected(portableGrep);
export const ls = selected(portableLs);
export const todo = selected(portableTodo);
export const storage = selected(portableStorage);
export const sandbox = selected(portableSandbox);

const webSearchTool = officialTool({
  name: "web_search",
  description: "Search the public web using Aex's managed search service.",
  input: z.object({
    query: z.string().min(1).max(500),
    num: z.number().int().min(1).max(10).default(5),
    country: z.string().min(2).max(8).optional(),
    language: z.string().min(2).max(16).optional(),
  }),
  output: z.object({
    query: z.string(),
    results: z.array(z.object({
      title: z.string(),
      url: z.string(),
      snippet: z.string(),
      date: z.string().optional(),
    })),
  }),
  capability: "aex.web.search",
});

const webFetchTool = officialTool({
  name: "web_fetch",
  description: "Fetch a public text, HTML, or JSON URL through Aex's guarded network service.",
  input: z.object({
    url: z.url(),
    max_chars: z.number().int().min(1).max(100_000).optional(),
  }),
  output: z.object({
    url: z.string(),
    status: z.number().int(),
    content_type: z.string(),
    text: z.string(),
    truncated: z.boolean(),
  }),
  capability: "aex.web.fetch",
});

export const webSearch = selected(webSearchTool);
export const webFetch = selected(webFetchTool);

/**
 * Let the agent create and explicitly interact with durable direct child sessions.
 */
export const subagents = selected(portableSubagents);
