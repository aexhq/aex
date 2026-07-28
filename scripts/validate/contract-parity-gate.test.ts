import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import { readWorkflow, workflowSteps } from "./workflow-test-helpers.js";

/**
 * The contract-parity gate must not go dormant again.
 *
 * `scripts/cicd/check-contract-parity.mjs` byte-compares the SSRF deny-list
 * classifier (`denyReasonForHostIp` .. `parseRemoteMcpTransport`) in
 * `packages/contracts/src/session-config.ts` against the platform repo's
 * `packages/shared/src/blueprint.ts`. That region is imperative validation
 * logic, so it is invisible to the Zod -> OpenAPI -> generated-types pipeline
 * (`openapi:check` / `openapi:types:check`): grep the generated
 * `packages/contracts/openapi/data-plane.json` for a deny reason and you get
 * zero hits. Nothing else in this repo asserts it.
 *
 * Until 2026-07-27 the gate was invoked by no workflow at all, so it read as
 * coverage while asserting nothing. These tests lock three things:
 *
 *   1. a workflow reaches it (transitively through a root package script);
 *   2. absence of the private tree SKIPS with a logged, specific reason;
 *   3. an explicitly configured PLATFORM_DIR that does not resolve FAILS —
 *      a misconfigured enforcement path must never look like an absent one.
 */

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const GATE = "scripts/cicd/check-contract-parity.mjs";
// Anchored at a path START: `apps/user-tests/scripts/shard-files.mjs` must be
// captured whole, not as the `scripts/shard-files.mjs` tail of itself.
const SCRIPT_FILE = /(?<![\w./-])(?:[\w.-]+\/)*scripts\/[\w./-]+\.(?:mjs|ts|js)\b/g;

type Scripts = Readonly<Record<string, string>>;

const rootScripts: Scripts =
  (
    JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8")) as {
      readonly scripts?: Scripts;
    }
  ).scripts ?? {};

const workflowDirectory = new URL("../../.github/workflows/", import.meta.url);

function workflowRunCommands(): readonly string[] {
  return readdirSync(workflowDirectory)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort()
    .flatMap((name) => Object.values(readWorkflow(`.github/workflows/${name}`).jobs))
    .flatMap((job) => workflowSteps(job))
    .map((step) => step.run)
    .filter((run): run is string => typeof run === "string");
}

/**
 * Root-manifest script names a command invokes as `bun run <name>`.
 * `--filter`/`--workspaces` invocations target a WORKSPACE manifest, not this
 * one, so they are excluded rather than resolved against the wrong scripts map.
 */
export function rootScriptCalls(command: string): readonly string[] {
  const tokens = command.split(/\s+/).filter((token) => token !== "");
  const names: string[] = [];
  for (let index = 0; index + 1 < tokens.length; index += 1) {
    if (tokens[index] !== "bun" || tokens[index + 1] !== "run") continue;
    let cursor = index + 2;
    let workspaceScoped = false;
    while (cursor < tokens.length && tokens[cursor]!.startsWith("-")) {
      if (tokens[cursor] === "--filter") {
        workspaceScoped = true;
        cursor += 2;
        continue;
      }
      if (tokens[cursor] === "--workspaces") workspaceScoped = true;
      cursor += 1;
    }
    const name = tokens[cursor];
    if (!workspaceScoped && name !== undefined) names.push(name);
  }
  return names;
}

/** Every `scripts/**` file reachable from the given entry commands. */
export function reachableScriptFiles(scripts: Scripts, entryCommands: readonly string[]): ReadonlySet<string> {
  const files = new Set<string>();
  const visited = new Set<string>();
  const queue = [...entryCommands];
  while (queue.length > 0) {
    const command = queue.shift()!;
    for (const match of command.matchAll(SCRIPT_FILE)) files.add(match[0]);
    for (const name of rootScriptCalls(command)) {
      if (visited.has(name)) continue;
      visited.add(name);
      const next = scripts[name];
      if (next !== undefined) queue.push(next);
    }
  }
  return files;
}

function referencedScriptFiles(commands: readonly string[]): readonly string[] {
  const referenced = new Set<string>();
  for (const command of commands) {
    for (const match of command.matchAll(SCRIPT_FILE)) referenced.add(match[0]);
  }
  return [...referenced].sort();
}

describe("contract-parity gate wiring", () => {
  it("is reached by a workflow through the root scripts graph", () => {
    const reachable = reachableScriptFiles(rootScripts, workflowRunCommands());
    // Non-vacuity: the traversal must find the gates already known to run.
    expect([...reachable]).toContain("scripts/cicd/check-public-boundary.mjs");
    expect([...reachable]).toContain("scripts/cicd/run-validation-tests.mjs");
    expect([...reachable]).toContain(GATE);
  });

  it("resolves a chained root script through to its files", () => {
    const files = reachableScriptFiles(
      {
        lint: "bun run brand:check && bun run --filter @aexhq/sdk build",
        "brand:check": "bun scripts/cicd/check-old-brand.mjs"
      },
      ["bun run lint"]
    );
    expect([...files]).toEqual(["scripts/cicd/check-old-brand.mjs"]);
  });

  it("captures a nested scripts path whole rather than as its own tail", () => {
    expect(referencedScriptFiles(["bun apps/user-tests/scripts/shard-files.mjs --matrix"])).toEqual([
      "apps/user-tests/scripts/shard-files.mjs"
    ]);
  });

  it("does not resolve a workspace-scoped invocation against the root manifest", () => {
    expect(rootScriptCalls("bun run --filter @aexhq/contracts build")).toEqual([]);
    expect(rootScriptCalls("bun run --workspaces --if-present test:unit")).toEqual([]);
    expect(rootScriptCalls("bun run lint && bun run typecheck")).toEqual(["lint", "typecheck"]);
  });

  // The general dead-gate detector: a script entry or workflow step naming a
  // file that does not exist is the same failure class as T18, one step worse.
  it("names no non-existent scripts/** file from any root script or workflow step", () => {
    const referenced = referencedScriptFiles([...Object.values(rootScripts), ...workflowRunCommands()]);
    expect(referenced.length).toBeGreaterThan(5);
    expect(referenced.filter((path) => !existsSync(resolve(repoRoot, path)))).toEqual([]);
  });
});

// --- gate behaviour, on a hermetic fixture pair -------------------------------

const PLATFORM_BLUEPRINT = [
  "const EGRESS_DENIED_RANGES = [];",
  "",
  "/**",
  " * Deny reasons shared with the public contracts mirror.",
  " * Surface tracked by server-side SSRF regression coverage.",
  " */",
  "export function denyReasonForHostIp(host) {",
  '  if (host === "127.0.0.1") return "must not target loopback (127.0.0.0/8)";',
  "  return null;",
  "}",
  "",
  "function parseRemoteMcpTransport(input) {",
  "  return input;",
  "}",
  ""
].join("\n");

// The public copy keeps the classifier module-private and drops the internal
// doc reference; both deltas are normalised away by the gate itself.
const PUBLIC_SESSION_CONFIG = PLATFORM_BLUEPRINT.replace(
  "export function denyReasonForHostIp",
  "function denyReasonForHostIp"
).replace(" * Surface tracked by server-side SSRF regression coverage.\n", "");

interface GateResult {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
}

function write(path: string, contents: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

function withGateFixture(
  platformBlueprint: string | null,
  body: (publicRoot: string, fixtureRoot: string) => void
): void {
  const fixtureRoot = mkdtempSync(join(tmpdir(), "aex-parity-gate-"));
  const publicRoot = join(fixtureRoot, "aex");
  try {
    write(join(publicRoot, GATE), readFileSync(resolve(repoRoot, GATE), "utf8"));
    write(join(publicRoot, "scripts", "cicd", "contract-parity-baseline.json"), '{\n  "entries": []\n}\n');
    write(join(publicRoot, "packages", "contracts", "src", "session-config.ts"), PUBLIC_SESSION_CONFIG);
    if (platformBlueprint !== null) {
      write(join(fixtureRoot, "platform", "packages", "shared", "src", "blueprint.ts"), platformBlueprint);
    }
    body(publicRoot, fixtureRoot);
  } finally {
    rmSync(fixtureRoot, { recursive: true, force: true });
  }
}

function runGate(publicRoot: string, overrides: Readonly<Record<string, string>> = {}): GateResult {
  const env: Record<string, string | undefined> = { ...process.env, ...overrides };
  if (!("PLATFORM_DIR" in overrides)) delete env.PLATFORM_DIR;
  try {
    const stdout = execFileSync(process.execPath, [join(publicRoot, GATE)], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      env
    });
    return { status: 0, stdout, stderr: "" };
  } catch (error) {
    const failure = error as {
      readonly status?: number;
      readonly stdout?: Buffer | string;
      readonly stderr?: Buffer | string;
    };
    return {
      status: failure.status ?? 1,
      stdout: String(failure.stdout ?? ""),
      stderr: String(failure.stderr ?? "")
    };
  }
}

describe("contract-parity gate behaviour", () => {
  it("enforces against a sibling platform tree and passes when the deny block agrees", () => {
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot) => {
      const result = runGate(publicRoot);
      expect(result.stderr).toBe("");
      expect(result.stdout).toContain("contract-parity passed");
      expect(result.status).toBe(0);
    });
  });

  it("fails on a deny-list divergence that is not in the baseline", () => {
    const drifted = PLATFORM_BLUEPRINT.replace("127.0.0.0/8", "127.0.0.0/9");
    withGateFixture(drifted, (publicRoot) => {
      const result = runGate(publicRoot);
      expect(result.stderr).toContain("contract-parity FAILED");
      expect(result.stderr).toContain("NEW divergence");
      expect(result.status).toBe(1);
    });
  });

  it("skips with exit 0 and names the path it probed when no platform tree exists", () => {
    withGateFixture(null, (publicRoot, fixtureRoot) => {
      const result = runGate(publicRoot);
      const probed = join(fixtureRoot, "platform").replaceAll("\\", "/");
      expect(result.stdout).toContain("SKIPPED");
      expect(result.stdout).toContain(probed);
      expect(result.stdout).toContain("PLATFORM_DIR");
      expect(result.status).toBe(0);
    });
  });

  it("fails closed when PLATFORM_DIR is set but holds no platform tree", () => {
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot, fixtureRoot) => {
      const missing = join(fixtureRoot, "not-a-platform-checkout");
      const result = runGate(publicRoot, { PLATFORM_DIR: missing });
      expect(result.stderr).toContain("PLATFORM_DIR");
      expect(result.stderr).toContain(missing.replaceAll("\\", "/"));
      expect(result.status).toBe(1);
    });
  });
});
