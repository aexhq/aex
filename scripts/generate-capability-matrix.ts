#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  checkRuntimeSupported,
  DEFAULT_RUN_PROVIDER,
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  selectRuntime,
  type PlatformRunSubmissionRequest,
  type RunProvider,
  type RuntimeKind
} from "../packages/contracts/src/submission.js";
import { RUN_MODELS_BY_PROVIDER } from "../packages/contracts/src/models.js";
import {
  PROVIDER_PUBLIC_SUPPORT,
  RUNTIME_VALIDATION_SUPPORT,
  type ProviderPublicSupport,
  type SupportPointer
} from "../packages/contracts/src/provider-support.js";

export const CAPABILITY_MATRIX_PATH = "packages/sdk/docs/provider-runtime-capabilities.md";

export interface RuntimeCapabilityCell {
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
  readonly defaultProvider: boolean;
  readonly autoRoute: RuntimeKind;
  readonly managedRuntime: RuntimeCapabilityCell;
}

function runtimeCell(provider: RunProvider, runtime: RuntimeKind, support: ProviderPublicSupport): RuntimeCapabilityCell {
  if (!checkRuntimeSupported(provider, runtime).ok) {
    throw new Error(`runtime ${runtime} is not supported for provider ${provider}`);
  }

  return {
    enforcement: "submission parser + managed dispatch",
    docsAnchor: support.docsAnchor,
    evidence: support.runtimeEvidence[runtime] ?? support.evidence
  };
}

function buildDispatcherProbe(provider: RunProvider): PlatformRunSubmissionRequest {
  return {
    workspaceId: "capability-matrix",
    idempotencyKey: `capability-matrix-${provider}`,
    credentialMode: "byok",
    provider,
    submission: {
      model: RUN_MODELS_BY_PROVIDER[provider][0]!,
      prompt: ["capability matrix runtime probe"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: {
      apiKey: `sk-${provider}-capability-matrix`
    }
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
      defaultProvider: provider === DEFAULT_RUN_PROVIDER,
      autoRoute: selectRuntime(buildDispatcherProbe(provider)),
      managedRuntime: runtimeCell(provider, "managed", publicSupport)
    };
  });
}

function renderPointerLinks(pointers: readonly SupportPointer[]): string {
  return pointers.map((pointer) => `[${pointer.label}](${pointer.href})`).join("; ");
}

function renderProviderLink(row: CapabilityMatrixRow): string {
  return `[${row.displayName}](#${row.docsAnchor})`;
}

function renderRuntimeCell(cell: RuntimeCapabilityCell): string {
  return `[managed](#${cell.docsAnchor})`;
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
  const runtimeList = RUNTIME_KINDS.map((runtime) => `\`${runtime}\``).join(", ");
  const lines = [
    "---",
    "title: Provider runtime capabilities",
    "---",
    "",
    "# Provider runtime capabilities",
    "",
    "Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/models.ts`; runtime routing is derived through `checkRuntimeSupported` and `selectRuntime` in `packages/contracts/src/submission.ts`.",
    "",
    "Regenerate with `pnpm capabilities:generate`; check with `pnpm capabilities:check`.",
    "",
    `Providers: ${providerList}. Runtime selectors: ${runtimeList}.`,
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
    "## Runtime routing",
    "",
    "| Provider | Default provider | Auto route | Runtime selector |",
    "| --- | --- | --- | --- |"
  );

  for (const row of rows) {
    lines.push(
      [
        `| \`${row.provider}\``,
        row.defaultProvider ? "yes" : "no",
        `\`${row.autoRoute}\``,
        `${renderRuntimeCell(row.managedRuntime)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "## Runtime evidence",
    "",
    "| Provider | Runtime | Enforcement path | Evidence |",
    "| --- | --- | --- | --- |"
  );

  for (const row of rows) {
    for (const runtime of RUNTIME_KINDS) {
      const cell = row.managedRuntime;
      lines.push(
        [
          `| \`${row.provider}\``,
          `\`${runtime}\``,
          cell.enforcement,
          `${renderPointerLinks(cell.evidence)} |`
        ].join(" | ")
      );
    }
  }

  lines.push(
    "",
    "## Validation errors",
    "",
    "| Code | Docs anchor | Enforcement path | Evidence |",
    "| --- | --- | --- | --- |"
  );

  for (const [code, support] of Object.entries(RUNTIME_VALIDATION_SUPPORT)) {
    lines.push(
      [
        `| \`${code}\``,
        `[${support.docsAnchor}](#${support.docsAnchor})`,
        support.enforcement,
        `${renderPointerLinks(support.evidence)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "### Managed unsupported features",
    "",
    "Provider-hosted skill refs (a `kind:\"provider\"` skill ref) are rejected because new runs dispatch to the managed runtime. Supply skill bytes through `Skill.fromFiles`, `Skill.fromPath`, `Skill.fromUrl`, or `Skill.fromCatalog`; each path normalizes to an asset that the platform snapshots into durable run asset storage.",
    "",
    "Notes:",
    "",
    "- Supported models are the public SDK model ids accepted for each provider.",
    "- Runtime routing describes how a validated submission is dispatched.",
    "- `runtime: \"native\"` is not a runtime selector; the submission parser rejects it as an invalid enum value.",
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
      `- Auto route: \`${row.autoRoute}\``,
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
    message: `${CAPABILITY_MATRIX_PATH} is stale; run pnpm capabilities:generate`
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
