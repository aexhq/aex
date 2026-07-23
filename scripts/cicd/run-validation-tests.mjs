#!/usr/bin/env node
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { realpathSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

// Runs the repository validation suite (scripts/validate) under `bun test`
// with the standard junit + assert-no-skips release gate. The suite runs with
// cwd pinned to the checkout's OWN scripts/validate directory so bun collects
// exactly that tree — a polluted self-hosted parent directory can contribute
// neither test files nor a bunfig (bun reads bunfig.toml from cwd only, never
// from ancestor directories).
//
// Serial by default: the validators scan Git state and spawn nested
// package-manager, compiler, and test processes; bun's default sequential
// file execution keeps those probes from starving one another (the successor
// of the retired vitest `fileParallelism: false` pin). The generous --timeout
// exists for the same reason — process-spawning tests, not slow logic.
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const validationRoot = resolve(repoRoot, "scripts", "validate");
const noSkipsGate = resolve(repoRoot, "scripts", "cicd", "assert-no-skips.mjs");
const reportPath = resolve(repoRoot, ".tmp", "junit-test-validate.xml");

for (const [label, path] of [
  ["root", validationRoot],
  ["gate", noSkipsGate]
]) {
  if (!existsSync(path)) {
    console.error(`validation-test-context: missing ${label}=${normalize(path)}`);
    process.exit(1);
  }
  assertInsideRepository(label, path);
}

console.log(
  [
    "validation-test-context:",
    `cwd=${normalize(process.cwd())}`,
    `repository=${normalize(realpathSync(repoRoot))}`,
    `root=${normalize(realpathSync(validationRoot))}`,
    `runner=bun-test`,
    `report=${normalize(reportPath)}`
  ].join(" ")
);

// A stale report from an earlier run must never satisfy the gate: bun writes
// NO outfile at all when it collects zero tests.
rmSync(reportPath, { force: true });
mkdirSync(dirname(reportPath), { recursive: true });

const bunCommand = "bun" in process.versions ? process.execPath : process.platform === "win32" ? "bun.exe" : "bun";
const result = spawnSync(
  bunCommand,
  ["test", "--isolate", "--timeout=120000", "--reporter=junit", `--reporter-outfile=${reportPath}`],
  { cwd: validationRoot, env: process.env, stdio: "inherit" }
);

if (result.error) {
  console.error(`validation-test-context: spawn failed: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  console.error(`validation-test-context: bun test terminated by signal ${result.signal}`);
  process.exit(1);
}

const gate = spawnSync(bunCommand, [noSkipsGate, reportPath], {
  cwd: repoRoot,
  env: process.env,
  stdio: "inherit"
});
if (gate.error) {
  console.error(`validation-test-context: no-skips gate spawn failed: ${gate.error.message}`);
  process.exit(1);
}
if (gate.signal) {
  console.error(`validation-test-context: no-skips gate terminated by signal ${gate.signal}`);
  process.exit(1);
}

// Test failure takes precedence; a green run must still pass the skip gate.
if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
process.exit(gate.status ?? 1);

function assertInsideRepository(label, path) {
  const rel = relative(repoRoot, path);
  if (rel === "" || (!isAbsolute(rel) && rel !== ".." && !rel.startsWith(`..${sep}`))) return;
  console.error(`validation-test-context: ${label} escaped repository=${normalize(path)}`);
  process.exit(1);
}

function normalize(path) {
  return path.replaceAll("\\", "/");
}
