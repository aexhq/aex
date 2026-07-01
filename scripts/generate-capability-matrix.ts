#!/usr/bin/env bun
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  RUN_PROVIDERS,
  type RunProvider
} from "../packages/contracts/src/submission.js";
import { RUN_MODELS_BY_PROVIDER } from "../packages/contracts/src/models.js";
import {
  PROVIDER_PUBLIC_SUPPORT,
  type ProviderPublicSupport,
  type SupportPointer
} from "../packages/contracts/src/provider-support.js";

export const CAPABILITY_MATRIX_PATH = "packages/sdk/docs/provider-runtime-capabilities.md";

export interface ManagedCapabilityCell {
  readonly enforcement: string;
  readonly docsAnchor: string;
  readonly evidence: readonly SupportPointer[];
}

export interface CapabilityMatrixRow {
  readonly provider: RunProvider;
  readonly displayName: string;
  readonly docsAnchor: string;
  readonly docs: readonly SupportPointer[];
  readonly evidence: readonly SupportPointer[];
  readonly managedExecution: ManagedCapabilityCell;
}

function managedCell(support: ProviderPublicSupport): ManagedCapabilityCell {
  return {
    enforcement: "submission parser + managed execution",
    docsAnchor: support.docsAnchor,
    evidence: support.managedEvidence
  };
}

function supportFor(provider: RunProvider): ProviderPublicSupport {
  return PROVIDER_PUBLIC_SUPPORT[provider];
}

export function buildCapabilityMatrixRows(): CapabilityMatrixRow[] {
  return RUN_PROVIDERS.map((provider) => {
    const publicSupport = supportFor(provider);
    return {
      provider,
      displayName: publicSupport.displayName,
      docsAnchor: publicSupport.docsAnchor,
      docs: publicSupport.docs,
      evidence: publicSupport.evidence,
      managedExecution: managedCell(publicSupport)
    };
  });
}

function renderPointerLinks(pointers: readonly SupportPointer[]): string {
  return pointers.map((pointer) => `[${pointer.label}](${pointer.href})`).join("; ");
}

function renderProviderLink(row: CapabilityMatrixRow): string {
  return `[${row.displayName}](#${row.docsAnchor})`;
}

function renderSupportedModels(provider: RunProvider): string {
  return RUN_MODELS_BY_PROVIDER[provider].map((model) => `\`${model}\``).join(", ");
}

export function renderProviderRuntimeCapabilityMarkdown(
  rows: readonly CapabilityMatrixRow[] = buildCapabilityMatrixRows()
): string {
  const providerList = rows
    .map((row) => `${renderProviderLink(row)} (\`${row.provider}\`)`)
    .join(", ");
  const lines = [
    "---",
    "title: Provider runtime capabilities",
    "---",
    "",
    "# Provider runtime capabilities",
    "",
    "Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/models.ts`.",
    "",
    "Regenerate with `bun run capabilities:generate`; check with `bun run capabilities:check`.",
    "",
    `Providers: ${providerList}.`,
    "",
    "All new submissions run on the managed runtime. Public support is expressed as supported providers and supported model ids.",
    "",
    "## Supported models",
    "",
    "| Provider | Selector | Supported models | Docs | Evidence |",
    "| --- | --- | --- | --- | --- |"
  ];

  for (const row of rows) {
    lines.push(
      [
        `| ${renderProviderLink(row)}`,
        `\`${row.provider}\``,
        renderSupportedModels(row.provider),
        renderPointerLinks(row.docs),
        `${renderPointerLinks(row.evidence)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "## Managed evidence",
    "",
    "| Provider | Enforcement path | Evidence |",
    "| --- | --- | --- |"
  );

  for (const row of rows) {
    const cell = row.managedExecution;
    lines.push(
      [
        `| \`${row.provider}\``,
        cell.enforcement,
        `${renderPointerLinks(cell.evidence)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "## Skills",
    "",
    "Skills are supplied as load-tools in the `tools` array. Build one with `Tools.fromSkillDir` or `Tools.fromSkillUrl`; each normalizes to an asset that the platform snapshots into durable run asset storage.",
    "",
    "Notes:",
    "",
    "- Supported models are the public SDK model ids accepted for each provider.",
    "- Execution uses the managed path; there is no public runtime selector.",
    "",
    "## Provider anchors",
    ""
  );

  for (const row of rows) {
    lines.push(
      `### ${row.displayName}`,
      "",
      `- Wire provider: \`${row.provider}\``,
      `- Supported models: ${renderSupportedModels(row.provider)}`,
      `- Docs: ${renderPointerLinks(row.docs)}`,
      `- Evidence: ${renderPointerLinks(row.evidence)}`,
      ""
    );
  }

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
  const expected = renderProviderRuntimeCapabilityMarkdown();

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
