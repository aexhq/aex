import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, setDefaultTimeout } from "bun:test";
// @ts-expect-error the gate is JS, and its text-splitting internals are asserted directly.
import { missingSignatures, splitComments } from "../cicd/check-contract-parity.mjs";
import { readWorkflow, workflowJob, workflowSteps, workflowTriggers } from "./workflow-test-helpers.js";

/**
 * The contract-parity gate must not go dormant again.
 *
 * `scripts/cicd/check-contract-parity.mjs` compares the SSRF deny-list
 * classifier (`denyReasonForHostIp` .. `parseRemoteMcpTransport`) in
 * `packages/contracts/src/session-config.ts` against the platform repo's
 * `packages/shared/src/blueprint.ts`. That region is imperative validation
 * logic, so it is invisible to the Zod -> OpenAPI -> generated-types pipeline
 * (`openapi:check` / `openapi:types:check`): grep the generated
 * `packages/contracts/openapi/data-plane.json` for a deny reason and you get
 * zero hits. Nothing else in this repo asserts it.
 *
 * Until 2026-07-27 the gate was invoked by no workflow at all. It was then
 * wired into `lint` — but `lint` checks out only this repository, so it has
 * always taken the SKIP path in CI and enforced on developer machines and
 * nowhere else. These tests lock what closes that:
 *
 *   1. a workflow reaches the gate (transitively through a root package script);
 *   2. a workflow dispatches the PRIVATE-side run, with the PR's merge SHA, and
 *      without ever executing pull-request code;
 *   3. absence of the private tree SKIPS by default, and FAILS under
 *      AEX_REQUIRE_PARITY=1;
 *   4. an explicitly configured PLATFORM_DIR that does not resolve FAILS — a
 *      misconfigured enforcement path must never look like an absent one;
 *   5. the comparison is pinned to an immutable platform commit, not a branch;
 *   6. comment wording is INFORMATION and executable text is the GATE;
 *   7. the baseline is a debt register — owner, expiry, and a ratchet that
 *      cannot grow.
 */

// Every gate case spawns the real script as a subprocess, and three of them
// also `git init` a fixture tree. On Windows that is comfortably past bun's
// 5s default; the work is real, not a hang.
setDefaultTimeout(60_000);

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const GATE = "scripts/cicd/check-contract-parity.mjs";
const DISPATCH_WORKFLOW = ".github/workflows/contract-parity.yml";
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

// --- the dispatch that makes public CI reach a tree it cannot check out -------

describe("private-side verification is dispatched from public CI", () => {
  const dispatch = readWorkflow(DISPATCH_WORKFLOW);
  const job = workflowJob(dispatch, "dispatch");

  it("triggers on pull_request_target, so a fork pull request reaches it too", () => {
    // `pull_request` cannot: a fork gets no secrets, so it could never mint the
    // token. `pull_request_target` runs the BASE branch's copy of this file with
    // base-repository credentials, which is the whole reason it exists.
    const triggers = Object.keys(workflowTriggers(dispatch));
    expect(triggers).toContain("pull_request_target");
    expect(triggers).toContain("push");
  });

  it("never checks out or executes pull-request code", () => {
    // The `pull_request_target` compromise, and the reason this job is a
    // dispatcher and not a verifier. A checkout of the PR head plus any install
    // or script step would hand a fork a write-scoped credential.
    for (const step of workflowSteps(job)) {
      expect(String(step.uses ?? ""), "no checkout in a pull_request_target job").not.toContain(
        "actions/checkout"
      );
    }
    const body = JSON.stringify(workflowSteps(job));
    for (const token of ["bun install", "bun ci", "npm install", "bun run "] as const) {
      expect(body, `${token} would execute pull-request code`).not.toContain(token);
    }
  });

  it("mints a token scoped to the private repository and nothing else", () => {
    const mint = workflowSteps(job).find((step) => String(step.uses ?? "").includes("create-github-app-token"));
    expect(mint, "the app-token step is missing").toBeDefined();
    expect(String(mint?.uses)).toMatch(/@[0-9a-f]{40}$/);
    expect((mint?.with as Record<string, string> | undefined)?.owner).toBe("aexhq");
    expect((mint?.with as Record<string, string> | undefined)?.repositories).toBe("platform");
  });

  it("sends the pull request's MERGE commit, not main and not the head", () => {
    // The canary is built from the merge result. `github.sha` on
    // pull_request_target is the BASE commit, so dispatching it would verify a
    // tree that is already merged — a version that never ships as this PR.
    const body = JSON.stringify(workflowSteps(job));
    expect(body).toContain("github.event.pull_request.merge_commit_sha");
    expect(body).toContain("client_payload[aex_sha]");
    expect(body).toContain("/repos/aexhq/platform/dispatches");
  });

  it("fails rather than passing when there is no mergeable commit to verify", () => {
    const resolveStep = workflowSteps(job).find((step) => step.id === "subject");
    expect(String(resolveStep?.run)).toContain("::error::");
    expect(String(resolveStep?.run)).toContain("exit 1");
  });

  it("holds no more permission than reading this repository", () => {
    expect(dispatch.permissions).toEqual({ contents: "read" });
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

// The public copy keeps the classifier module-private and re-words the internal
// doc reference. The `export` delta is normalised away by the gate; the doc
// wording is now dropped by comment stripping rather than by a bespoke rule.
const PUBLIC_SESSION_CONFIG = PLATFORM_BLUEPRINT.replace(
  "export function denyReasonForHostIp",
  "function denyReasonForHostIp"
).replace(" * Surface tracked by server-side SSRF regression coverage.", " * Surface tracked privately.");

interface GateResult {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
}

function write(path: string, contents: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
}

const EMPTY_BASELINE = '{\n  "maxEntries": 0,\n  "entries": []\n}\n';

function withGateFixture(
  platformBlueprint: string | null,
  body: (publicRoot: string, fixtureRoot: string) => void,
  baseline: string = EMPTY_BASELINE
): void {
  const fixtureRoot = mkdtempSync(join(tmpdir(), "aex-parity-gate-"));
  const publicRoot = join(fixtureRoot, "aex");
  try {
    write(join(publicRoot, GATE), readFileSync(resolve(repoRoot, GATE), "utf8"));
    write(join(publicRoot, "scripts", "cicd", "contract-parity-baseline.json"), baseline);
    write(join(publicRoot, "packages", "contracts", "src", "session-config.ts"), PUBLIC_SESSION_CONFIG);
    if (platformBlueprint !== null) {
      write(join(fixtureRoot, "platform", "packages", "shared", "src", "blueprint.ts"), platformBlueprint);
    }
    body(publicRoot, fixtureRoot);
  } finally {
    rmSync(fixtureRoot, { recursive: true, force: true });
  }
}

/** Turn the fixture platform tree into a real git checkout, and return its HEAD. */
function commitPlatformFixture(fixtureRoot: string): string {
  const dir = join(fixtureRoot, "platform");
  const git = (...args: string[]): string =>
    execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  git("init", "--quiet");
  git("config", "user.email", "fixture@aex.test");
  git("config", "user.name", "fixture");
  git("add", "-A");
  git("commit", "--quiet", "--no-gpg-sign", "-m", "fixture");
  return git("rev-parse", "HEAD").trim();
}

function runGate(publicRoot: string, overrides: Readonly<Record<string, string>> = {}): GateResult {
  const env: Record<string, string | undefined> = { ...process.env, ...overrides };
  for (const key of ["PLATFORM_DIR", "PLATFORM_REF", "AEX_REQUIRE_PARITY"]) {
    if (!(key in overrides)) delete env[key];
  }
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

  it("fails on a behavioural divergence that is not in the baseline", () => {
    const drifted = PLATFORM_BLUEPRINT.replace("127.0.0.0/8", "127.0.0.0/9");
    withGateFixture(drifted, (publicRoot) => {
      const result = runGate(publicRoot);
      expect(result.stderr).toContain("contract-parity FAILED");
      expect(result.stderr).toContain("NEW behavioural divergence");
      expect(result.status).toBe(1);
    });
  });

  it("treats a comment-only divergence as information, not as a gate failure", () => {
    // This is the whole reason the baseline had nine entries and none of them
    // described a behavioural difference. Gating on exact equality of a region
    // that is mostly prose manufactures a file nobody reads.
    const reworded = PLATFORM_BLUEPRINT.replace(
      " * Deny reasons shared with the public contracts mirror.",
      " * Deny reasons, kept in parity with the public mirror. See the infra backlog."
    );
    withGateFixture(reworded, (publicRoot) => {
      const result = runGate(publicRoot);
      expect(result.stdout).toContain("line-level difference");
      expect(result.stdout).toContain("INFORMATION, not the gate");
      expect(result.stdout).toContain("contract-parity passed");
      expect(result.status).toBe(0);
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

  it("fails instead of skipping under AEX_REQUIRE_PARITY=1", () => {
    // In a job that IS the gate, a missing platform tree is a misconfiguration.
    // Exit 0 there is indistinguishable from the gate passing, which is exactly
    // how this check spent its life reading as coverage while asserting nothing.
    withGateFixture(null, (publicRoot) => {
      const result = runGate(publicRoot, { AEX_REQUIRE_PARITY: "1" });
      expect(result.stderr).toContain("AEX_REQUIRE_PARITY=1");
      expect(result.stderr).toContain("no platform tree");
      expect(result.status).toBe(1);
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

describe("the comparison names an exact platform commit", () => {
  it("requires PLATFORM_REF in enforcement mode", () => {
    // Without the pin the gate proves compatibility with whatever happened to be
    // checked out. The release consumes an exact commit, so the gate names one.
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot, fixtureRoot) => {
      const result = runGate(publicRoot, {
        AEX_REQUIRE_PARITY: "1",
        PLATFORM_DIR: join(fixtureRoot, "platform")
      });
      expect(result.stderr).toContain("requires PLATFORM_REF");
      expect(result.status).toBe(1);
    });
  });

  it("refuses a branch name, because a branch tip is a moving object", () => {
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot, fixtureRoot) => {
      const result = runGate(publicRoot, {
        PLATFORM_DIR: join(fixtureRoot, "platform"),
        PLATFORM_REF: "main"
      });
      expect(result.stderr).toContain("not a 40-character commit SHA");
      expect(result.status).toBe(1);
    });
  });

  it("refuses a pin that does not match the tree it was handed", () => {
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot, fixtureRoot) => {
      commitPlatformFixture(fixtureRoot);
      const result = runGate(publicRoot, {
        PLATFORM_DIR: join(fixtureRoot, "platform"),
        PLATFORM_REF: "0".repeat(40)
      });
      expect(result.stderr).toContain("but");
      expect(result.stderr).toContain("is at");
      expect(result.status).toBe(1);
    });
  });

  it("passes when the pin matches, and reports the commit it compared", () => {
    withGateFixture(PLATFORM_BLUEPRINT, (publicRoot, fixtureRoot) => {
      const head = commitPlatformFixture(fixtureRoot);
      const result = runGate(publicRoot, {
        AEX_REQUIRE_PARITY: "1",
        PLATFORM_DIR: join(fixtureRoot, "platform"),
        PLATFORM_REF: head
      });
      expect(result.stdout).toContain("contract-parity passed");
      expect(result.stdout).toContain(head);
      expect(result.status).toBe(0);
    });
  });
});

describe("the baseline is a debt register, not an allowlist", () => {
  const key = {
    scope: "deny-list",
    side: "platform",
    lineHash: "0".repeat(64)
  };

  function baselineFile(entries: readonly unknown[], maxEntries = entries.length): string {
    return `${JSON.stringify({ maxEntries, entries }, null, 2)}\n`;
  }

  function isoDaysFromNow(days: number): string {
    return new Date(Date.now() + days * 86_400_000).toISOString().slice(0, 10);
  }

  it("is empty today, with a ratchet of zero", () => {
    // The committed state, asserted here so shipping a non-empty baseline is a
    // visible decision rather than a quiet one.
    const committed = JSON.parse(
      readFileSync(resolve(repoRoot, "scripts/cicd/contract-parity-baseline.json"), "utf8")
    ) as { maxEntries: number; entries: unknown[] };
    expect(committed.entries).toEqual([]);
    expect(committed.maxEntries).toBe(0);
  });

  it("refuses an entry with no owner or expiry", () => {
    withGateFixture(
      PLATFORM_BLUEPRINT,
      (publicRoot) => {
        const result = runGate(publicRoot);
        expect(result.stderr).toContain("owner");
        expect(result.stderr).toContain("permanent debt");
        expect(result.status).toBe(1);
      },
      baselineFile([{ ...key, why: "because" }])
    );
  });

  it("refuses an expired entry, which is the mechanism working", () => {
    withGateFixture(
      PLATFORM_BLUEPRINT,
      (publicRoot) => {
        const result = runGate(publicRoot);
        expect(result.stderr).toContain("EXPIRED");
        expect(result.status).toBe(1);
      },
      baselineFile([
        {
          ...key,
          why: "because",
          owner: "maintainer",
          recordedAt: isoDaysFromNow(-20),
          expiresAt: isoDaysFromNow(-1)
        }
      ])
    );
  });

  it("refuses a window wider than the 30-day ceiling", () => {
    withGateFixture(
      PLATFORM_BLUEPRINT,
      (publicRoot) => {
        const result = runGate(publicRoot);
        expect(result.stderr).toContain("the ceiling is 30");
        expect(result.status).toBe(1);
      },
      baselineFile([
        {
          ...key,
          why: "because",
          owner: "maintainer",
          recordedAt: isoDaysFromNow(0),
          expiresAt: isoDaysFromNow(90)
        }
      ])
    );
  });

  it("refuses a count above its own ratchet", () => {
    withGateFixture(
      PLATFORM_BLUEPRINT,
      (publicRoot) => {
        const result = runGate(publicRoot);
        expect(result.stderr).toContain("maxEntries ratchet is 0");
        expect(result.status).toBe(1);
      },
      baselineFile(
        [
          {
            ...key,
            why: "because",
            owner: "maintainer",
            recordedAt: isoDaysFromNow(0),
            expiresAt: isoDaysFromNow(10)
          }
        ],
        0
      )
    );
  });

  it("will not let --update raise the ratchet", () => {
    // Admitting a NEW divergence has to be a reviewed hand edit, the way adding
    // a quarantine entry is. A tool that can widen its own allowance is not a
    // ratchet.
    const drifted = PLATFORM_BLUEPRINT.replace("127.0.0.0/8", "127.0.0.0/9");
    withGateFixture(drifted, (publicRoot) => {
      const env: Record<string, string | undefined> = { ...process.env };
      for (const k of ["PLATFORM_DIR", "PLATFORM_REF", "AEX_REQUIRE_PARITY"]) delete env[k];
      let stderr = "";
      let status = 0;
      try {
        execFileSync(process.execPath, [join(publicRoot, GATE), "--update"], {
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
          env
        });
      } catch (error) {
        const failure = error as { status?: number; stderr?: Buffer | string };
        status = failure.status ?? 1;
        stderr = String(failure.stderr ?? "");
      }
      expect(stderr).toContain("above the maxEntries ratchet");
      expect(status).toBe(1);
      // And the baseline on disk is untouched.
      const after = readFileSync(join(publicRoot, "scripts", "cicd", "contract-parity-baseline.json"), "utf8");
      expect(JSON.parse(after).entries).toEqual([]);
    });
  });
});

describe("comment stripping is sound enough to gate on", () => {
  it("removes comments and keeps code", () => {
    const { code, comments } = splitComments(
      ["// leading", "const a = 1; // trailing", "/* block", "   more */", "const b = 2;"].join("\n")
    ) as { code: string; comments: string };
    expect(code).toContain("const a = 1;");
    expect(code).toContain("const b = 2;");
    expect(code).not.toContain("trailing");
    expect(code).not.toContain("more");
    expect(comments).toContain("leading");
    expect(comments).toContain("more");
  });

  it("does not mistake a URL inside a string for a line comment", () => {
    // The reason this is a state machine and not a regex: `"https://x"` contains
    // `//`, and stripping from there would delete real code to end of line.
    const { code } = splitComments('const u = "https://example.test/a"; const v = 2;') as { code: string };
    expect(code).toContain("https://example.test/a");
    expect(code).toContain("const v = 2;");
  });

  it("leaves a regex literal alone", () => {
    const { code } = splitComments("const re = /^::(?:ffff:)?(\\d{1,3})$/.exec(host);") as { code: string };
    expect(code).toContain("/^::(?:ffff:)?(\\d{1,3})$/.exec(host)");
  });

  it("detects a stripped body that lost the code it exists to compare", () => {
    const raw = "function denyReasonForHostIp(h) { return null; }";
    expect(missingSignatures(raw, raw)).toEqual([]);
    expect(missingSignatures(raw, "")).toEqual(["denyReasonForHostIp"]);
  });
});
