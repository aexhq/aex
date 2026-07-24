#!/usr/bin/env bun
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { MODEL_SLUG_PATTERN } from "../packages/contracts/src/models.js";

export const CAPABILITY_MATRIX_PATH = "packages/sdk/docs/provider-runtime-capabilities.md";

/**
 * Under the managed Vercel AI Gateway there is no provider axis and no closed
 * model catalog: a model id is an open `creator/model` gateway slug that the
 * platform routes with its single managed key. This doc therefore describes the
 * MODEL-access contract (slug shape + streaming + runtime independence) rather
 * than a per-provider capability matrix.
 */
export function renderModelCapabilityMarkdown(): string {
  const lines = [
    "---",
    "title: Model access",
    "---",
    "",
    "# Model access",
    "",
    "Generated from `packages/contracts/src/models.ts`.",
    "",
    "Regenerate with `bun run capabilities:generate`; check with `bun run capabilities:check`.",
    "",
    "Aex routes every model through the managed Vercel AI Gateway. You name a model",
    "by its gateway `creator/model` **slug** and the platform's single managed key",
    "handles the upstream provider relationship — you never supply a provider API",
    "key, and there is no `provider` field.",
    "",
    "## Model ids are gateway slugs",
    "",
    `- A model id is a \`creator/model\` slug string, validated at the boundary by \`parseModelSlug\` against \`${MODEL_SLUG_PATTERN.source}\` (lowercase creator, then \`/\`, then the model segment).`,
    "- Examples: `anthropic/claude-haiku-4-5`, `anthropic/claude-sonnet-4-6`, `deepseek/deepseek-v4-flash`, `openai/gpt-4.1`, `google/gemini-2.5-flash`.",
    "- The catalog is OPEN: adding a model the gateway serves needs zero code — a well-formed slug just works. A slug this SDK does not recognize is still accepted at the boundary and arbitrated by the gateway at submit time; a truly unknown model fails there.",
    "",
    "## Streaming",
    "",
    "`outputMode: \"stream\"` is honored for ALL models — every model streams through the gateway. There is no per-model streaming-capability gate.",
    "",
    "## Skills",
    "",
    "Skills are supplied through the top-level `skills` option. Build one with `Skill.fromDir`, `Skill.fromUrl`, `Skill.fromFiles`, `Skill.fromContent`, or `Skill.fromBytes`; each normalizes to a named workspace skill that the platform snapshots into durable session asset storage.",
    "",
    "## Runtime selection is independent of model",
    "",
    "Runtime selection is independent of the model: `runtime.kind` accepts `container`, `spot_container`, or `lambda`; `runtime.size` accepts the managed size presets.",
    ""
  ];
  return lines.join("\n");
}

function normalizeGeneratedMarkdown(value: string): string {
  return value.replace(/\r\n/g, "\n");
}

export function checkCapabilityMatrix(current: string, expected: string): { readonly ok: true } | { readonly ok: false; readonly message: string } {
  if (normalizeGeneratedMarkdown(current) === normalizeGeneratedMarkdown(expected)) return { ok: true };
  return {
    ok: false,
    message: `${CAPABILITY_MATRIX_PATH} is stale; run bun run capabilities:generate`
  };
}

async function main(): Promise<void> {
  const args = new Set(process.argv.slice(2));
  const outPath = resolve(process.cwd(), CAPABILITY_MATRIX_PATH);
  const expected = renderModelCapabilityMarkdown();

  if (args.has("--write")) {
    await writeFile(outPath, expected, "utf8");
    process.stdout.write(`wrote ${CAPABILITY_MATRIX_PATH}\n`);
    return;
  }

  if (args.has("--check")) {
    const current = await readFile(outPath, "utf8");
    const result = checkCapabilityMatrix(current, expected);
    if (!result.ok) {
      process.stderr.write(`${result.message}\n`);
      process.exit(1);
    }
    process.stdout.write(`${CAPABILITY_MATRIX_PATH} is up to date\n`);
    return;
  }

  process.stdout.write(expected);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  await main();
}
