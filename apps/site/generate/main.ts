import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

interface RouteRow {
  readonly operationId: string;
  readonly method: string;
  readonly path: string;
  readonly summary: string;
  readonly fragment: string;
  /**
   * Present when the contract declares the operation and nothing serves it.
   *
   * The docs publish the fact and not this string. Every reason in the ledger
   * is an engineering note written for engineers, and one that has gone stale
   * is worse to publish than no sentence at all.
   */
  readonly deferredReason?: string;
}

interface CliRow { readonly path: string; readonly routeId: string; readonly deferred: boolean }

/** What a reader sees in place of an operation that is not built yet. */
const NOT_YET_AVAILABLE = "**Not yet available.** This operation is published in the contract and answers `501 not_implemented`.";

const workspaceRoot = resolve(import.meta.dir, "../../..");

export function generateSiteViews(outputRoot = resolve(import.meta.dir, "../.generated")): void {
  const lock = JSON.parse(readFileSync(resolve(workspaceRoot, "api/generated/bundle.lock.json"), "utf8")) as {
    contractDigest: string;
  };
  const routes = (JSON.parse(
    readFileSync(resolve(workspaceRoot, "api/generated/registries/routes.json"), "utf8"),
  ) as { routes: RouteRow[] }).routes.sort((left: RouteRow, right: RouteRow) =>
    left.operationId.localeCompare(right.operationId));
  const identity = Object.freeze({
    generatedBy: "apps/site/generate/main.ts",
    contractDigest: lock.contractDigest,
    sdkVersion: "0.50.0",
    cliVersion: "0.50.0",
  });
  const header = `<!-- ${JSON.stringify(identity)} -->\n`;
  const api = `${header}# HTTP API\n\n${routes.map((route) => `## \`${route.operationId}\`\n\n\`${route.method} ${route.path}\` — ${route.summary}\n${route.deferredReason ? `\n${NOT_YET_AVAILABLE}\n` : ""}`).join("\n")}`;
  const cliRows = readCliRegistry();
  const cli = `${header}# CLI\n\n${cliRows.map((row) => `- \`aex ${row.path}\` → \`${row.routeId}\`${row.deferred ? " (not yet available)" : ""}`).join("\n")}\n`;
  const sdkSurface = { ...identity, exports: ["Aex", "AexApiError", "AexTransport", "ROUTES", "RouteId"].sort() };
  const search = {
    ...identity,
    documents: routes.map((route) => ({ id: route.operationId, title: route.summary, deferred: Boolean(route.deferredReason) })),
  };
  mkdirSync(outputRoot, { recursive: true });
  writeFileSync(resolve(outputRoot, "api-reference.md"), api);
  writeFileSync(resolve(outputRoot, "cli-reference.md"), cli);
  writeFileSync(resolve(outputRoot, "not-yet-available.md"), notYetAvailable(header, routes));
  writeFileSync(resolve(outputRoot, "sdk-surface.json"), `${JSON.stringify(sdkSurface, null, 2)}\n`);
  writeFileSync(resolve(outputRoot, "search-index.json"), `${JSON.stringify(search, null, 2)}\n`);
  writeFileSync(
    resolve(outputRoot, "llms.txt"),
    `${header}AEX public API\n\n${routes.map((route) => `${route.operationId}: ${route.summary}${route.deferredReason ? " (not yet available)" : ""}`).join("\n")}\n`,
  );
}

/**
 * One page answering "what is missing?", grouped by authoring fragment.
 *
 * Scanning the reference for the marker works and is the wrong shape for that
 * question: a reader who wants the answer wants a list, not a search.
 */
function notYetAvailable(header: string, routes: readonly RouteRow[]): string {
  const deferred = routes.filter((route) => route.deferredReason);
  const fragments = [...new Set(deferred.map((route) => route.fragment))].sort();
  const sections = fragments.map((fragment) => {
    const rows = deferred
      .filter((route) => route.fragment === fragment)
      .map((route) => `- \`${route.operationId}\` — \`${route.method} ${route.path}\``)
      .join("\n");
    return `## ${fragment}\n\n${rows}\n`;
  });
  return `${header}# Not yet available\n\nThese ${deferred.length} operations are published in the contract and answer \`501 not_implemented\`. Everything else in the reference is served.\n\n${sections.join("\n")}`;
}

function readCliRegistry(): CliRow[] {
  const result = Bun.spawnSync(
    ["cargo", "run", "-q", "-p", "aex-cli", "--", "--dump-command-registry"],
    {
      cwd: workspaceRoot,
      env: { ...process.env, CARGO_BUILD_JOBS: "4", CARGO_TARGET_DIR: resolve(workspaceRoot, ".tmp/clients-target") },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  if (result.exitCode !== 0) throw new Error(`CLI registry generation failed: ${result.stderr.toString()}`);
  return (JSON.parse(result.stdout.toString()) as CliRow[]).sort((left: CliRow, right: CliRow) =>
    left.path.localeCompare(right.path));
}

if (import.meta.main) generateSiteViews();
