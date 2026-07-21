import { resolve as resolvePath } from "node:path";
import type { CliIO } from "../internal.js";
import type { StartArguments } from "./start-arguments.js";
import { portableBasename } from "./command-primitives.js";
import {
  buildCliFile,
  buildCliInstructions,
  buildCliSkill,
  buildCliTool,
  type CliFileDraft,
  type CliInstructionsDraft,
  type CliSkillDraft,
  type CliToolDraft
} from "./start-submit.js";

export interface StartAttachments {
  readonly skills: readonly CliSkillDraft[];
  readonly tools: readonly CliToolDraft[];
  readonly instructions: readonly CliInstructionsDraft[];
  readonly files: readonly CliFileDraft[];
}

/** Build local attachment drafts in the historical group/read order. */
export async function buildStartAttachments(io: CliIO, args: StartArguments): Promise<StartAttachments> {
  const skills = await Promise.all(args.skills.map((ref) => buildSkill(io, ref)));
  const tools = await Promise.all(args.tools.map((ref) => buildTool(io, ref)));
  const instructions = await Promise.all(args.instructions.map((ref) => buildInstructions(io, ref)));
  const files = await Promise.all(args.files.map((ref) => buildFile(io, ref)));
  return { skills, tools, instructions, files };
}

async function buildSkill(io: CliIO, ref: string): Promise<CliSkillDraft> {
  return buildCliSkill(await readAtFile(io, ref), ref);
}

async function buildTool(io: CliIO, ref: string): Promise<CliToolDraft> {
  const content = await readAtFile(io, ref);
  const entry = portableBasename(stripAt(ref));
  const name = deriveName(ref, 1);
  return buildCliTool({ name, description: `Custom tool ${name}`, entry, content });
}

async function buildInstructions(io: CliIO, ref: string): Promise<CliInstructionsDraft> {
  return buildCliInstructions(await readAtFile(io, ref), deriveName(ref, 2));
}

async function buildFile(io: CliIO, ref: string): Promise<CliFileDraft> {
  if (!io.readFileBytes) throw new Error("binary file reads are unavailable in this CLI host");
  const bytes = await io.readFileBytes(resolvePath(io.cwd(), stripAt(ref)));
  return buildCliFile({ name: portableBasename(stripAt(ref)), bytes });
}

async function readAtFile(io: CliIO, value: string): Promise<string> {
  return io.readFile(resolvePath(io.cwd(), stripAt(value)));
}

function stripAt(value: string): string {
  return value.startsWith("@") ? value.slice(1) : value;
}

function deriveName(ref: string, minLen: number): string {
  const base = portableBasename(stripAt(ref));
  const noExt = base.includes(".") ? base.slice(0, base.lastIndexOf(".")) : base;
  let slug = noExt.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  while (slug.length < minLen) slug += "x";
  return slug;
}
