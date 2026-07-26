import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

// Per-file test isolation guard (successor of the retired vitest
// config-isolation lock). The retired vitest configs guaranteed one fresh
// module registry per test file; under bun that guarantee exists ONLY while
// every `bun test` invocation carries `--isolate` (one process per file).
// Losing the flag anywhere silently reintroduces cross-file module-state
// bleed, so every invocation site is locked here:
//   - package.json scripts that run `bun test` directly;
//   - package.json scripts that run the user-tests lane wrapper (it forwards
//     its argv to `bun test` verbatim, so the flag must ride in the script);
//   - the validation runner's own assembled argv.

const BUN_TEST_SEGMENT = /(?:^|\s)bun(?:\.exe)?\s+test(?=\s|$)/;
const LANE_WRAPPER_SEGMENT = /(?:^|\s)bun\s+scripts\/user-bun-test\.mjs(?=\s|$)/;
const ISOLATE_FLAG = /(?:^|\s)--isolate(?=\s|$)/;

interface ManifestScripts {
  readonly manifest: string;
  readonly scripts: Readonly<Record<string, string>>;
}

export function missingIsolateInvocations(entries: readonly ManifestScripts[]): readonly string[] {
  const offenders: string[] = [];
  for (const { manifest, scripts } of entries) {
    for (const [name, command] of Object.entries(scripts)) {
      // Chained commands: every `bun test` / lane-wrapper segment must carry
      // the flag itself; a flag on a sibling segment proves nothing.
      for (const segment of command.split("&&").map((part) => part.trim())) {
        if (!BUN_TEST_SEGMENT.test(segment) && !LANE_WRAPPER_SEGMENT.test(segment)) continue;
        if (!ISOLATE_FLAG.test(segment)) offenders.push(`${manifest} ${name}: ${segment}`);
      }
    }
  }
  return offenders;
}

function workspaceManifests(): readonly ManifestScripts[] {
  const manifests: ManifestScripts[] = ["package.json"]
    .concat(
      ["packages", "apps"].flatMap((group) =>
        readdirSync(resolve(repoRoot, group), { withFileTypes: true })
          .filter((entry) => entry.isDirectory())
          .map((entry) => `${group}/${entry.name}/package.json`)
      )
    )
    .filter((manifest) => existsSync(resolve(repoRoot, manifest)))
    .map((manifest) => ({
      manifest,
      scripts:
        (JSON.parse(readFileSync(resolve(repoRoot, manifest), "utf8")) as {
          readonly scripts?: Readonly<Record<string, string>>;
        }).scripts ?? {}
    }));
  return manifests;
}

describe("bun test per-file isolation", () => {
  it("keeps --isolate on every bun test and lane-wrapper script in the workspace", () => {
    const manifests = workspaceManifests();
    const testInvocations = manifests.flatMap(({ manifest, scripts }) =>
      Object.values(scripts)
        .flatMap((command) => command.split("&&"))
        .filter((segment) => BUN_TEST_SEGMENT.test(segment) || LANE_WRAPPER_SEGMENT.test(segment))
        .map(() => manifest)
    );

    // The scan may never go vacuous: the flipped packages and the user-tests
    // lanes are known bun-test invokers.
    expect(testInvocations.length).toBeGreaterThanOrEqual(10);
    expect(missingIsolateInvocations(manifests)).toEqual([]);
  });

  it("keeps --isolate in the validation runner's assembled bun test argv", () => {
    const source = readFileSync(resolve(repoRoot, "scripts/cicd/run-validation-tests.mjs"), "utf8");
    expect(source).toMatch(/\["test",\s*"--isolate"/);
  });

  it("flags a bun test segment that drops --isolate", () => {
    expect(
      missingIsolateInvocations([
        {
          manifest: "packages/example/package.json",
          scripts: {
            "test:unit":
              "mkdir -p .tmp && bun test --reporter=junit --reporter-outfile=.tmp/junit.xml && node gate.mjs .tmp/junit.xml"
          }
        }
      ])
    ).toEqual([
      "packages/example/package.json test:unit: bun test --reporter=junit --reporter-outfile=.tmp/junit.xml"
    ]);
  });

  it("flags a lane-wrapper segment that drops --isolate", () => {
    expect(
      missingIsolateInvocations([
        {
          manifest: "apps/user-tests/package.json",
          scripts: { "test:user:heavy": "bun scripts/user-bun-test.mjs --timeout=900000 test/live/x.test.ts" }
        }
      ])
    ).toEqual([
      "apps/user-tests/package.json test:user:heavy: bun scripts/user-bun-test.mjs --timeout=900000 test/live/x.test.ts"
    ]);
  });

  it("does not miscount flags on sibling chained segments", () => {
    expect(
      missingIsolateInvocations([
        {
          manifest: "packages/example/package.json",
          scripts: { test: "echo --isolate && bun test suite" }
        }
      ])
    ).toEqual(["packages/example/package.json test: bun test suite"]);
    expect(
      missingIsolateInvocations([
        {
          manifest: "packages/example/package.json",
          scripts: { test: "bun test --isolate suite && bun run other" }
        }
      ])
    ).toEqual([]);
  });
});
