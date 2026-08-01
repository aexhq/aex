import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

interface RouteRow {
  readonly operationId: string;
  readonly method: string;
  readonly path: string;
  readonly summary: string;
}

interface CliRow { readonly path: string; readonly routeId: string }

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
  const api = `${header}# HTTP API\n\n${routes.map((route) => `## \`${route.operationId}\`\n\n\`${route.method} ${route.path}\` — ${route.summary}\n`).join("\n")}`;
  const cliRows = readCliRegistry();
  const cli = `${header}# CLI\n\n${cliRows.map((row) => `- \`aex ${row.path}\` → \`${row.routeId}\``).join("\n")}\n`;
  const sdkSurface = { ...identity, exports: ["Aex", "AexApiError", "AexTransport", "ROUTES", "RouteId"].sort() };
  const search = { ...identity, documents: routes.map((route) => ({ id: route.operationId, title: route.summary })) };
  mkdirSync(outputRoot, { recursive: true });
  writeFileSync(resolve(outputRoot, "api-reference.md"), api);
  writeFileSync(resolve(outputRoot, "cli-reference.md"), cli);
  writeFileSync(resolve(outputRoot, "sdk-surface.json"), `${JSON.stringify(sdkSurface, null, 2)}\n`);
  writeFileSync(resolve(outputRoot, "search-index.json"), `${JSON.stringify(search, null, 2)}\n`);
  writeFileSync(resolve(outputRoot, "llms.txt"), `${header}AEX public API\n\n${routes.map((route) => `${route.operationId}: ${route.summary}`).join("\n")}\n`);
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
