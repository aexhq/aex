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
import {
  PROVIDER_CAPABILITY,
  type NativeAgentCapability
} from "../packages/contracts/src/provider-capability.js";
import {
  PROVIDER_SUPPORT_STATUSES,
  PROVIDER_PUBLIC_SUPPORT,
  type ProviderPublicSupport,
  type ProviderSupportStatus,
  type SupportPointer
} from "../packages/contracts/src/provider-support.js";

export const CAPABILITY_MATRIX_PATH = "packages/sdk/docs/provider-runtime-capabilities.md";

export type RuntimeSupportStatus = ProviderSupportStatus;
export type NativeFeatureSupportStatus = ProviderSupportStatus | "n/a";

export interface RuntimeCapabilityCell {
  readonly status: RuntimeSupportStatus;
  readonly ownership: ProviderSupportStatus;
  readonly enforcement: string;
  readonly docsAnchor: string;
  readonly evidence: readonly SupportPointer[];
}

export interface CapabilityMatrixRow {
  readonly provider: RunProvider;
  readonly displayName: string;
  readonly publicStatus: ProviderSupportStatus;
  readonly docsAnchor: string;
  readonly docs: readonly SupportPointer[];
  readonly evidence: readonly SupportPointer[];
  readonly defaultProvider: boolean;
  readonly autoRoute: RuntimeKind;
  readonly nativeRuntime: RuntimeCapabilityCell;
  readonly managedRuntime: RuntimeCapabilityCell;
  readonly nativeExecutor: string;
  readonly inlineSkills: NativeFeatureSupportStatus;
  readonly files: NativeFeatureSupportStatus;
  readonly mcpServers: NativeFeatureSupportStatus;
}

function runtimeCell(provider: RunProvider, runtime: RuntimeKind, support: ProviderPublicSupport): RuntimeCapabilityCell {
  const supported = checkRuntimeSupported(provider, runtime).ok;
  if (!supported) {
    return {
      status: "rejected",
      ownership: "rejected",
      enforcement: "checkRuntimeSupported runtime_native_unsupported",
      docsAnchor: support.docsAnchor,
      evidence: support.evidence
    };
  }

  const evidence = support.runtimeEvidence[runtime] ?? support.evidence;
  const status = support.status === "supported" ? "supported" : "live-unverified";
  return {
    status,
    ownership: runtime === "native" ? "provider-inherited" : status,
    enforcement: runtime === "managed" ? "submission parser + Goose Managed dispatch" : "submission parser + provider-native dispatch",
    docsAnchor: support.docsAnchor,
    evidence
  };
}

function nativeFeatureStatus(
  nativeAgent: NativeAgentCapability | null,
  feature: keyof NativeAgentCapability["serves"]
): NativeFeatureSupportStatus {
  if (!nativeAgent) return "n/a";
  return nativeAgent.serves[feature] ? "supported" : "rejected";
}

function buildDispatcherProbe(provider: RunProvider): PlatformRunSubmissionRequest {
  return {
    workspaceId: "capability-matrix",
    idempotencyKey: `capability-matrix-${provider}`,
    credentialMode: "byok",
    provider,
    submission: {
      model: "capability-matrix-model",
      prompt: ["capability matrix runtime probe"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: {
      [provider]: { apiKey: `sk-${provider}-capability-matrix` }
    } as PlatformRunSubmissionRequest["secrets"]
  };
}

function supportFor(provider: RunProvider): ProviderPublicSupport {
  return PROVIDER_PUBLIC_SUPPORT[provider];
}

export function buildCapabilityMatrixRows(): CapabilityMatrixRow[] {
  return RUN_PROVIDERS.map((provider) => {
    const publicSupport = supportFor(provider);
    const nativeAgent = PROVIDER_CAPABILITY[provider].nativeAgent;
    return {
      provider,
      displayName: publicSupport.displayName,
      publicStatus: publicSupport.status,
      docsAnchor: publicSupport.docsAnchor,
      docs: publicSupport.docs,
      evidence: publicSupport.evidence,
      defaultProvider: provider === DEFAULT_RUN_PROVIDER,
      autoRoute: selectRuntime(buildDispatcherProbe(provider)),
      nativeRuntime: runtimeCell(provider, "native", publicSupport),
      managedRuntime: runtimeCell(provider, "managed", publicSupport),
      nativeExecutor: nativeAgent?.executor ?? "n/a",
      inlineSkills: nativeFeatureStatus(nativeAgent, "inlineSkills"),
      files: nativeFeatureStatus(nativeAgent, "files"),
      mcpServers: nativeFeatureStatus(nativeAgent, "mcpServers")
    };
  });
}

function renderPointerLinks(pointers: readonly SupportPointer[]): string {
  return pointers.map((pointer) => `[${pointer.label}](${pointer.href})`).join("; ");
}

function renderProviderLink(row: CapabilityMatrixRow): string {
  return `[${row.displayName}](#${row.docsAnchor})`;
}

function renderCodeOrText(value: string): string {
  return value === "n/a" ? value : `\`${value}\``;
}

function renderRuntimeCell(cell: RuntimeCapabilityCell): string {
  const status = `[${cell.status}](#${cell.docsAnchor})`;
  return cell.ownership === cell.status ? status : `${status}; ${cell.ownership}`;
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
    "Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/provider-capability.ts`; runtime cells are derived through `checkRuntimeSupported` and `selectRuntime` in `packages/contracts/src/submission.ts`.",
    "",
    "Regenerate with `pnpm capabilities:generate`; check with `pnpm capabilities:check`.",
    "",
    `Providers: ${providerList}. Runtime selectors: ${runtimeList}.`,
    "",
    "Public support facts are listed separately from runtime routing facts. Goose Managed is the universal managed runtime. A provider-native runtime is used only when the shared capability registry declares one.",
    "",
    `Status vocabulary: ${PROVIDER_SUPPORT_STATUSES.map((status) => `\`${status}\``).join(", ")}.`,
    "",
    "## Public support",
    "",
    "| Provider | Wire value | Status | Docs | Evidence |",
    "| --- | --- | --- | --- | --- |"
  ];

  for (const row of rows) {
    lines.push(
      [
        `| ${renderProviderLink(row)}`,
        `\`${row.provider}\``,
        row.publicStatus,
        renderPointerLinks(row.docs),
        `${renderPointerLinks(row.evidence)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "## Runtime routing",
    "",
    "| Provider | Default provider | Auto route | `runtime: \"native\"` | `runtime: \"managed\"` | Native executor |",
    "| --- | --- | --- | --- | --- | --- |"
  );

  for (const row of rows) {
    lines.push(
      [
        `| \`${row.provider}\``,
        row.defaultProvider ? "yes" : "no",
        `\`${row.autoRoute}\``,
        renderRuntimeCell(row.nativeRuntime),
        renderRuntimeCell(row.managedRuntime),
        `${renderCodeOrText(row.nativeExecutor)} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "## Runtime cell evidence",
    "",
    "| Provider | Runtime | Status | Ownership | Enforcement path | Evidence |",
    "| --- | --- | --- | --- | --- | --- |"
  );

  for (const row of rows) {
    for (const runtime of RUNTIME_KINDS) {
      const cell = runtime === "native" ? row.nativeRuntime : row.managedRuntime;
      lines.push(
        [
          `| \`${row.provider}\``,
          `\`${runtime}\``,
          cell.status,
          cell.ownership,
          cell.enforcement,
          `${renderPointerLinks(cell.evidence)} |`
        ].join(" | ")
      );
    }
  }

  lines.push(
    "",
    "## Native feature parity",
    "",
    "| Provider | Native inline skills | Native files | Native MCP servers |",
    "| --- | --- | --- | --- |"
  );

  for (const row of rows) {
    lines.push(
      [
        `| \`${row.provider}\``,
        row.inlineSkills,
        row.files,
        `${row.mcpServers} |`
      ].join(" | ")
    );
  }

  lines.push(
    "",
    "Notes:",
    "",
    "- Public status describes provider availability on the SDK surface. Runtime routing describes how a validated submission is dispatched.",
    "- `rejected` for `runtime: \"native\"` means the submission parser returns `runtime_native_unsupported` for that provider.",
    "- `live-unverified` means the shape is accepted by code but lacks equal live user evidence in this repository.",
    "- `provider-inherited` means antpath passes a capability through while the provider or customer-controlled service owns final behavior.",
    "- Native feature parity cells come from `PROVIDER_CAPABILITY[provider].nativeAgent.serves`; `n/a` means the provider has no native agent runtime.",
    "",
    "## Provider anchors",
    ""
  );

  for (const row of rows) {
    lines.push(
      `### ${row.displayName}`,
      "",
      `- Wire provider: \`${row.provider}\``,
      `- Public status: ${row.publicStatus}`,
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
