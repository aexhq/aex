import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  CAPABILITY_MATRIX_PATH,
  checkCapabilityMatrix,
  renderModelCapabilityMarkdown
} from "../generate-capability-matrix.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

/**
 * Retired managed-gateway surface. Since the 2026-07-24 managed-gateway pivot the
 * API returns 400 `invalid_submission` for `secrets.apiKeys` / `apiKeys`
 * (`platform/infra/lambdas/src/api.ts` submission-shape gate), the CLI rejects a
 * `--<provider>-api-key` flag, `Models.*` and `missing_provider_key` no longer
 * exist, and BYOK is not a product. A published example that teaches any of them
 * is a quickstart that cannot run — the B4 defect this gate exists to prevent.
 */
const RETIRED_DOC_SURFACE = [
  { name: "retired `apiKeys` submission field / SDK option", pattern: /\bapiKeys\b/ },
  { name: "retired BYOK product claim", pattern: /\bBYOK\b|bring[-\s]your[-\s]own[-\s]key/i },
  {
    name: "retired `--<provider>-api-key` CLI flag (the aex `--api-key` flag is fine)",
    pattern: /--(?!api-key\b)[a-z0-9-]+-api-key\b/
  },
  { name: "retired `missing_provider_key` error code", pattern: /\bmissing_provider_key\b/ },
  { name: "deleted `Models.*` typed const", pattern: /\bModels\.[A-Z][A-Z0-9_]*/ }
] as const;

/**
 * Every published doc surface that can carry a runnable example. `packages/sdk/docs`
 * is the SOURCE both docs sites generate from, so covering it covers the generated
 * `content/docs/{guides,concepts}` trees even in a checkout that has not run
 * `docs:generate` yet.
 */
const PUBLIC_DOC_ROOTS = [
  "README.md",
  "SECURITY.md",
  "packages/sdk/README.md",
  "packages/sdk/docs",
  "apps/docs/content/docs"
] as const;

/**
 * The private platform repo mirrors these docs into the dashboard site. It is a
 * sibling checkout in the combined workspace and absent from a public-only CI
 * checkout, so it is scanned when reachable and never required: the mirror is
 * generated wholly from `PUBLIC_DOC_ROOTS`, which are covered unconditionally.
 */
const PLATFORM_MIRROR_ROOT = resolve(repoRoot, process.env["AEX_PLATFORM_REPO_DIR"] ?? "../platform");
const PLATFORM_DOC_ROOTS = ["apps/dashboard/content/docs"] as const;

function walkDocFiles(root: string): readonly string[] {
  if (!existsSync(root)) return [];
  if (statSync(root).isFile()) return [root];
  const out: string[] = [];
  const stack = [root];
  while (stack.length > 0) {
    const dir = stack.pop()!;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      const abs = resolve(dir, entry.name);
      if (entry.isDirectory()) stack.push(abs);
      else if (entry.isFile() && (entry.name.endsWith(".md") || entry.name.endsWith(".json"))) out.push(abs);
    }
  }
  return out.sort();
}

function scannedDocFiles(): readonly string[] {
  return [
    ...PUBLIC_DOC_ROOTS.flatMap((root) => walkDocFiles(resolve(repoRoot, root))),
    ...PLATFORM_DOC_ROOTS.flatMap((root) => walkDocFiles(resolve(PLATFORM_MIRROR_ROOT, root)))
  ];
}

function label(file: string): string {
  const fromPublic = relative(repoRoot, file);
  if (!fromPublic.startsWith("..")) return fromPublic.split(sep).join("/");
  return `<platform>/${relative(PLATFORM_MIRROR_ROOT, file).split(sep).join("/")}`;
}

describe("managed-gateway model-access capability doc", () => {
  it("renders a provider-free, gateway-slug model-access doc", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).toContain("# Model access");
    expect(rendered).toContain("managed Vercel AI Gateway");
    expect(rendered).toContain("parseModelSlug");
    expect(rendered).toContain("anthropic/claude-haiku-4-5");
    // No provider AXIS survives: no provider selector field, no closed catalog
    // table, no dropped providers, no BYOK. (The word "provider" may still appear
    // in explanatory prose — "you never supply a provider API key".)
    expect(rendered).not.toContain("openrouter");
    expect(rendered).not.toContain("doubao");
    expect(rendered).not.toContain("| Provider |");
    expect(rendered).not.toContain("BYOK");
    expect(rendered).not.toContain("apiKeys");
    expect(rendered).not.toMatch(/provider:\s*Providers\./);
  });

  it("states streaming is honored for all models (no capability gate)", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).toMatch(/honored for ALL models/);
  });

  it("keeps deploy and economic caveats out of the generated doc", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).not.toMatch(/\b(billing|bills|cost|costs|margin|margins|topology)\b/i);
  });

  it("fails the check when committed output is stale", () => {
    const expected = renderModelCapabilityMarkdown();
    expect(checkCapabilityMatrix(expected, expected)).toEqual({ ok: true });
    expect(checkCapabilityMatrix("stale\n", expected)).toEqual({
      ok: false,
      message: `${CAPABILITY_MATRIX_PATH} is stale; run bun run capabilities:generate`
    });
  });

  it("keeps the committed markdown in sync", () => {
    const current = readFileSync(resolve(repoRoot, CAPABILITY_MATRIX_PATH), "utf8");
    expect(checkCapabilityMatrix(current, renderModelCapabilityMarkdown())).toEqual({ ok: true });
  });
});

describe("published docs teach only the managed-gateway contract", () => {
  it("covers every doc surface a quickstart can be published from", () => {
    const files = scannedDocFiles().map(label);
    // Fail loudly if a root is renamed away instead of silently going green.
    for (const root of PUBLIC_DOC_ROOTS) {
      expect(existsSync(resolve(repoRoot, root)), `public doc root missing: ${root}`).toBe(true);
    }
    for (const required of [
      "README.md",
      "SECURITY.md",
      "packages/sdk/README.md",
      "packages/sdk/docs/quickstart.md",
      "packages/sdk/docs/public-surface.json",
      "apps/docs/content/docs/examples.md",
      "apps/docs/content/docs/integrations.md"
    ]) {
      expect(files, `unscanned quickstart surface: ${required}`).toContain(required);
    }
  });

  it("has no retired provider-key surface in any scanned doc", () => {
    const files = scannedDocFiles();
    expect(files.length).toBeGreaterThan(0);

    const offenders: string[] = [];
    for (const file of files) {
      const lines = readFileSync(file, "utf8").split(/\r?\n/);
      lines.forEach((line, index) => {
        for (const entry of RETIRED_DOC_SURFACE) {
          if (entry.pattern.test(line)) {
            offenders.push(`${label(file)}:${index + 1} has ${entry.name}: ${line.trim()}`);
          }
        }
      });
    }
    expect(offenders).toEqual([]);
  });

  it("scans the private dashboard docs mirror whenever that repo is checked out", () => {
    // Not a skip: a public-only CI checkout has no sibling platform repo, and the
    // assertion above still holds for it, because the mirror is generated wholly
    // from PUBLIC_DOC_ROOTS. When the mirror IS reachable it must be in the scan.
    const mirrorRoots = PLATFORM_DOC_ROOTS.map((root) => resolve(PLATFORM_MIRROR_ROOT, root));
    const reachableRoots = mirrorRoots.filter((root) => existsSync(root));
    const mirrorFiles = mirrorRoots.flatMap((root) => walkDocFiles(root));
    expect(mirrorFiles.length > 0).toBe(reachableRoots.length > 0);
    const scanned = scannedDocFiles();
    expect(mirrorFiles.filter((file) => !scanned.includes(file))).toEqual([]);
  });
});
