import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

// Bun version policy (bun-test migration Wave 0). One release is pinned at
// every pin site — packageManager, engines.bun, and CI setup-bun steps — and
// the sites may never disagree. `bun test --parallel` is additionally gated on
// the crashed-worker fix: before 1.3.14 a crashed parallel worker could be
// reported as a passing shard, silently masking failures. Pin the whole
// fleet, not one manifest.
const PINNED_BUN_VERSION = "1.3.14";
const PARALLEL_SAFE_BUN_FLOOR = "1.3.14";

const BUN_TEST_INVOCATION = /(?:^|\s)bun(?:\.exe)?(?:\s+--\S+)*\s+test(?=\s|$)/;
const PARALLEL_FLAG = /(?:^|\s)--parallel(?:=|\s|$)/;

describe("bun version policy", () => {
  it("pins every packageManager and engines.bun site to the same bun release", () => {
    const rootManifest = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8")) as {
      readonly packageManager?: string;
      readonly engines?: Readonly<Record<string, string>>;
    };

    // The root manifest must carry both pin sites so discovery can never go vacuous.
    expect(rootManifest.packageManager).toBe(`bun@${PINNED_BUN_VERSION}`);
    expect(rootManifest.engines?.bun).toBe(`>=${PINNED_BUN_VERSION}`);

    const pins = collectManifestPins();
    expect(pins.length).toBeGreaterThan(0);
    for (const pin of pins) {
      expect(
        pin.version,
        `${pin.path} ${pin.site} disagrees with the pinned bun release ${PINNED_BUN_VERSION}`
      ).toBe(PINNED_BUN_VERSION);
    }
  });

  it("pins every CI setup-bun step to the same bun release", () => {
    const { pins, setupBunSteps } = collectCiPins();

    expect(setupBunSteps).toBeGreaterThan(0);
    expect(pins.length).toBeGreaterThan(0);
    for (const pin of pins) {
      expect(
        pin.version,
        `${pin.path} (${pin.site}) disagrees with the pinned bun release ${PINNED_BUN_VERSION}`
      ).toBe(PINNED_BUN_VERSION);
    }
  });

  it("distinguishes `bun test --parallel` from `bun run --parallel` script commands", () => {
    expect(usesParallelBunTest("bun test --parallel")).toBe(true);
    expect(usesParallelBunTest("bun test --isolate --parallel --reporter=junit --reporter-outfile=report.xml")).toBe(true);
    expect(usesParallelBunTest("bun run build && bun test --parallel ./src")).toBe(true);
    expect(usesParallelBunTest("bun --bun test --parallel")).toBe(true);

    expect(usesParallelBunTest("bun test")).toBe(false);
    expect(usesParallelBunTest("bun run --workspaces --if-present --parallel lint")).toBe(false);
    expect(usesParallelBunTest("bun run --filter @aexhq/user-tests --parallel test:user")).toBe(false);
    expect(usesParallelBunTest("bun test-conformance/run.mjs --parallel")).toBe(false);
    expect(usesParallelBunTest("bunx vitest run --parallel")).toBe(false);
  });

  it("admits `bun test --parallel` scripts only under the crashed-worker-safe bun floor", () => {
    const parallelSites = collectParallelBunTestScripts();

    for (const site of parallelSites) {
      expect(
        versionAtLeast(PINNED_BUN_VERSION, PARALLEL_SAFE_BUN_FLOOR),
        `${site.path} script "${site.name}" uses \`bun test --parallel\`, which masks crashed workers before bun ${PARALLEL_SAFE_BUN_FLOOR}; the pinned bun ${PINNED_BUN_VERSION} is too old`
      ).toBe(true);

      const disagreeing = [...collectManifestPins(), ...collectCiPins().pins].filter(
        (pin) => pin.version !== PINNED_BUN_VERSION
      );
      expect(
        disagreeing,
        `${site.path} script "${site.name}" uses \`bun test --parallel\` while bun pin sites disagree; every pin site must guarantee >=${PARALLEL_SAFE_BUN_FLOOR}`
      ).toEqual([]);
    }

    // Runtime leg: when any `bun test --parallel` script exists and this
    // process itself runs on bun, that executing bun must carry the fix too.
    const executingBun = process.versions.bun;
    const executingBunMeetsFloor =
      parallelSites.length === 0 ||
      executingBun === undefined ||
      versionAtLeast(executingBun, PARALLEL_SAFE_BUN_FLOOR);
    expect(
      executingBunMeetsFloor,
      `executing bun ${executingBun ?? "(not bun)"} predates the crashed-worker fix required by \`bun test --parallel\` scripts`
    ).toBe(true);
  });
});

interface VersionPin {
  readonly path: string;
  readonly site: string;
  readonly version: string;
}

interface ParallelScriptSite {
  readonly path: string;
  readonly name: string;
  readonly command: string;
}

function trackedPackageManifests(): string[] {
  const listing = execFileSync("git", ["ls-files", "-z", "--", "package.json", "*/package.json"], {
    cwd: repoRoot,
    encoding: "utf8"
  });
  return listing
    .split("\0")
    .filter((path) => path === "package.json" || path.endsWith("/package.json"));
}

function collectManifestPins(): VersionPin[] {
  const pins: VersionPin[] = [];

  for (const path of trackedPackageManifests()) {
    const manifest = JSON.parse(readFileSync(resolve(repoRoot, path), "utf8")) as {
      readonly packageManager?: string;
      readonly engines?: Readonly<Record<string, string>>;
    };

    if (manifest.packageManager !== undefined) {
      pins.push({
        path,
        site: "packageManager",
        version: extractVersion(manifest.packageManager, /^bun@(\d+\.\d+\.\d+)$/, `${path} packageManager`)
      });
    }

    const enginesBun = manifest.engines?.bun;
    if (enginesBun !== undefined) {
      pins.push({
        path,
        site: "engines.bun",
        version: extractVersion(enginesBun, /^>=(\d+\.\d+\.\d+)$/, `${path} engines.bun`)
      });
    }
  }

  return pins;
}

function ciSetupFiles(): string[] {
  const files: string[] = [];

  const workflowsDir = resolve(repoRoot, ".github", "workflows");
  for (const entry of readdirSync(workflowsDir)) {
    if (/\.ya?ml$/.test(entry)) files.push(`.github/workflows/${entry}`);
  }

  const actionsDir = resolve(repoRoot, ".github", "actions");
  if (existsSync(actionsDir)) {
    for (const entry of readdirSync(actionsDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      for (const name of ["action.yml", "action.yaml"]) {
        if (existsSync(resolve(actionsDir, entry.name, name))) {
          files.push(`.github/actions/${entry.name}/${name}`);
        }
      }
    }
  }

  return files;
}

function collectCiPins(): { pins: VersionPin[]; setupBunSteps: number } {
  const pins: VersionPin[] = [];
  let setupBunSteps = 0;

  for (const path of ciSetupFiles()) {
    const contents = readFileSync(resolve(repoRoot, path), "utf8");

    const uses = contents.match(/^[ \t]*-?[ \t]*uses:[ \t]*["']?oven-sh\/setup-bun@/gm) ?? [];
    setupBunSteps += uses.length;

    const envDefinitions = [...contents.matchAll(/^[ \t]*BUN_VERSION:[ \t]*(\S+)[ \t]*$/gm)].map((match) =>
      unquote(matchGroup(match, `${path} BUN_VERSION`))
    );
    for (const definition of envDefinitions) {
      pins.push({ path, site: "BUN_VERSION env", version: definition });
    }

    const pinValues = [
      ...contents.matchAll(/bun-version:[ \t]*("[^"]*"|'[^']*'|\$\{\{[^}]*\}\}|[^\s,}]+)/g)
    ];
    if (pinValues.length !== uses.length) {
      throw new Error(
        `${path}: found ${uses.length} oven-sh/setup-bun steps but ${pinValues.length} bun-version pins; every setup-bun step must pin bun-version exactly once`
      );
    }

    for (const match of pinValues) {
      const raw = unquote(matchGroup(match, `${path} bun-version`));
      if (raw.startsWith("${{")) {
        if (!/^\$\{\{\s*env\.BUN_VERSION\s*\}\}$/.test(raw)) {
          throw new Error(`${path}: bun-version may only defer to env.BUN_VERSION; got ${raw}`);
        }
        if (envDefinitions.length === 0) {
          throw new Error(`${path}: bun-version references env.BUN_VERSION but the file defines no BUN_VERSION`);
        }
        continue; // Agreement is enforced through the BUN_VERSION definition pin above.
      }
      pins.push({ path, site: "bun-version", version: raw });
    }
  }

  return { pins, setupBunSteps };
}

function collectParallelBunTestScripts(): ParallelScriptSite[] {
  const sites: ParallelScriptSite[] = [];

  for (const path of trackedPackageManifests()) {
    const manifest = JSON.parse(readFileSync(resolve(repoRoot, path), "utf8")) as {
      readonly scripts?: Readonly<Record<string, string>>;
    };
    for (const [name, command] of Object.entries(manifest.scripts ?? {})) {
      if (usesParallelBunTest(command)) sites.push({ path, name, command });
    }
  }

  return sites;
}

function usesParallelBunTest(command: string): boolean {
  return command
    .split(/&&|\|\||;|\|/)
    .some((segment) => BUN_TEST_INVOCATION.test(segment) && PARALLEL_FLAG.test(segment));
}

function versionAtLeast(candidate: string, floor: string): boolean {
  const [candidateMajor, candidateMinor, candidatePatch] = parseVersion(candidate);
  const [floorMajor, floorMinor, floorPatch] = parseVersion(floor);

  if (candidateMajor !== floorMajor) return candidateMajor > floorMajor;
  if (candidateMinor !== floorMinor) return candidateMinor > floorMinor;
  return candidatePatch >= floorPatch;
}

function parseVersion(value: string): readonly [number, number, number] {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(value);
  if (!match) throw new Error(`unparseable bun version: ${value}`);
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

function extractVersion(value: string, pattern: RegExp, context: string): string {
  const version = pattern.exec(value)?.[1];
  if (version === undefined) {
    throw new Error(`${context}: expected ${String(pattern)} to match ${JSON.stringify(value)}`);
  }
  return version;
}

function matchGroup(match: RegExpMatchArray, context: string): string {
  const group = match[1];
  if (group === undefined) throw new Error(`${context}: capture group missing in ${JSON.stringify(match[0])}`);
  return group;
}

function unquote(value: string): string {
  if (
    value.length >= 2 &&
    ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))
  ) {
    return value.slice(1, -1);
  }
  return value;
}
