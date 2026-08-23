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
// The former brain.storage kernel intrinsic as an ordinary hosted capability: same
// model-facing contract, served by Aex over Brain's session-scoped storage API.
const storageKey = z.string().min(1).max(1024);
const storagePath = z.string().min(1).max(4096);
const storageGeneration = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/);

const storageTool = officialTool({
  name: "storage",
  description: "Explicitly save, load, or list durable files for this session.",
  input: z.discriminatedUnion("action", [
    z.object({
      action: z.literal("save"),
      key: storageKey,
      source: z.discriminatedUnion("kind", [
        z.object({ kind: z.literal("sandbox_path"), path: storagePath, generation: storageGeneration }),
        z.object({ kind: z.literal("inline_text"), text: z.string().max(94_208) }),
      ]),
      overwrite: z.boolean().optional(),
    }),
    z.object({
      action: z.literal("load"),
      key: storageKey,
      path: storagePath,
      generation: storageGeneration,
      overwrite: z.boolean().optional(),
    }),
    z.object({
      action: z.literal("list"),
      prefix: z.string().max(1024).optional(),
      cursor: z.string().max(4096).optional(),
      limit: z.number().int().positive().max(100).optional(),
    }),
  ]),
  capability: "aex.storage",
});

export const storage = selected(storageTool);
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
