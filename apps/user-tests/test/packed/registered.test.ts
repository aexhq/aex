import { afterAll, beforeAll, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { USER_SCENARIOS } from "../../scenarios.js";
import sdkManifest from "../../../../packages/sdk/package.json" with { type: "json" };

const packageRoot = resolve(import.meta.dir, "../../../../packages/sdk");
const suiteRoot = mkdtempSync(join(tmpdir(), "aex-packed-sdk-"));
const tarball = join(suiteRoot, "aexhq-sdk.tgz");
const bun = "bun" in process.versions ? process.execPath : "bun";
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

beforeAll(() => {
  run(bun, ["pm", "pack", "--filename", tarball, "--quiet"], packageRoot);
}, 120_000);

afterAll(() => {
  rmSync(suiteRoot, { force: true, recursive: true });
});

test("all packed scenarios declare an external-consumer artifact", () => {
  const scenarios = USER_SCENARIOS.filter(({ suite }) => suite === "packed");
  expect(scenarios.length).toBeGreaterThan(0);
  for (const scenario of scenarios) expect(scenario.artifacts.some((artifact) => artifact === "sdk" || artifact === "cli")).toBe(true);
});

test("packed.sdk-install-node installs and executes the packed root surface with Node", () => {
  const evidence = installAndProbe("node", npm, [
    "install",
    "--offline",
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
    "--cache",
    join(suiteRoot, "npm-cache"),
  ]);

  expect(evidence.runtime).toBe("node");
  expect(evidence.packageVersion).toBe(sdkManifest.version);
}, 120_000);

test("packed.sdk-install-bun installs and executes the packed root surface with Bun", () => {
  const evidence = installAndProbe("bun", bun, [
    "install",
    "--offline",
    "--ignore-scripts",
    "--no-progress",
    "--cache-dir",
    join(suiteRoot, "bun-cache"),
  ]);

  expect(evidence.runtime).toBe("bun");
  expect(evidence.packageVersion).toBe(sdkManifest.version);
}, 120_000);

interface ProbeEvidence {
  readonly runtime: "node" | "bun";
  readonly packageVersion: string;
}

function installAndProbe(runtime: ProbeEvidence["runtime"], installer: string, installArguments: readonly string[]): ProbeEvidence {
  const consumer = join(suiteRoot, `${runtime}-consumer`);
  mkdirSync(consumer);
  writeFileSync(join(consumer, "package.json"), `${JSON.stringify({
    name: `aex-packed-${runtime}-consumer`,
    private: true,
    type: "module",
    dependencies: { "@aexhq/sdk": `file:${tarball.replaceAll("\\", "/")}` },
  }, null, 2)}\n`);
  writeFileSync(join(consumer, "probe.mjs"), probeSource(runtime));

  run(installer, installArguments, consumer);
  const executable = runtime === "node" ? "node" : bun;
  return JSON.parse(run(executable, ["probe.mjs"], consumer)) as ProbeEvidence;
}

function probeSource(runtime: ProbeEvidence["runtime"]): string {
  return `
import * as sdk from "@aexhq/sdk";

const operationId = sdk.newId("operation");
if (!sdk.isId("operation", operationId) || sdk.assertId("operation", operationId) !== operationId) {
  throw new Error("packed SDK identifier behavior mismatch");
}
const ranges = sdk.planDownloadRanges(sdk.MAX_SINGLE_GET_BYTES + 1);
if (ranges.length !== 2 || ranges[1]?.start !== sdk.MAX_SINGLE_GET_BYTES) {
  throw new Error(\`packed SDK behavior mismatch: \${JSON.stringify(ranges)}\`);
}
if (sdk.regionalHost("euw1") !== "https://eu-west-1.api.aex.dev") {
  throw new Error("packed SDK routing behavior mismatch");
}
console.log(JSON.stringify({ runtime: ${JSON.stringify(runtime)}, packageVersion: sdk.Aex.buildInfo().packageVersion }));
`.trimStart();
}

function run(executable: string, arguments_: readonly string[], cwd: string): string {
  return execFileSync(executable, arguments_, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, CI: "true" },
    timeout: 120_000,
    windowsHide: true,
  }).trim();
}
