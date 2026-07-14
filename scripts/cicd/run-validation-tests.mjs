#!/usr/bin/env node
import { existsSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const validationRoot = resolve(repoRoot, "scripts", "validate");
const configPath = resolve(validationRoot, "vitest.config.ts");
const vitestEntrypoint = resolve(repoRoot, "node_modules", "vitest", "vitest.mjs");

for (const [label, path] of [
  ["config", configPath],
  ["vitest", vitestEntrypoint]
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
    `config=${normalize(realpathSync(configPath))}`,
    `vitest=${normalize(realpathSync(vitestEntrypoint))}`
  ].join(" ")
);

const result = spawnSync(
  process.execPath,
  ["run", "vitest", "run", "--config", configPath],
  { cwd: repoRoot, env: process.env, stdio: "inherit" }
);

if (result.error) {
  console.error(`validation-test-context: spawn failed: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  console.error(`validation-test-context: vitest terminated by signal ${result.signal}`);
  process.exit(1);
}
process.exit(result.status ?? 1);

function assertInsideRepository(label, path) {
  const rel = relative(repoRoot, path);
  if (rel === "" || (!isAbsolute(rel) && rel !== ".." && !rel.startsWith(`..${sep}`))) return;
  console.error(`validation-test-context: ${label} escaped repository=${normalize(path)}`);
  process.exit(1);
}

function normalize(path) {
  return path.replaceAll("\\", "/");
}
