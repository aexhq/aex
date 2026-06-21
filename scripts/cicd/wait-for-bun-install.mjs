#!/usr/bin/env bun

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [packageName, version] = process.argv.slice(2);

if (!packageName || !version) {
  console.error("Usage: wait-for-bun-install.mjs <package> <version>");
  process.exit(2);
}

const registry = (process.env.NPM_REGISTRY_URL ?? "https://registry.npmjs.org").replace(/\/$/, "");
const timeoutMs = Number(process.env.BUN_INSTALL_WAIT_TIMEOUT_MS ?? 10 * 60_000);
const intervalMs = Number(process.env.BUN_INSTALL_WAIT_INTERVAL_MS ?? 10_000);
const deadline = Date.now() + timeoutMs;
const spec = `${packageName}@${version}`;
const bunCommand = "bun" in process.versions ? process.execPath : "bun";
let attempt = 0;
let lastStatus = "not checked";

while (Date.now() < deadline) {
  attempt++;
  const result = await tryInstall(spec, registry, false);
  if (result.exitCode === 0) {
    console.log(`${spec} is installable with bun.`);
    process.exit(0);
  }

  lastStatus = summarize(result);
  console.warn(`bun install attempt ${attempt} for ${spec} failed; retrying. Last status: ${lastStatus}`);
  await tryInstall(spec, registry, true);
  await sleep(intervalMs);
}

console.error(`Timed out waiting for ${spec} to install with bun. Last status: ${lastStatus}`);
process.exit(1);

async function tryInstall(spec, registry, refreshCache) {
  const installDir = await mkdtemp(join(tmpdir(), "aex-bun-install-check-"));
  try {
    await writeFile(
      join(installDir, "package.json"),
      JSON.stringify({ name: "aex-bun-install-check", version: "0.0.0", private: true }, null, 2)
    );

    const args = ["install", spec, "--ignore-scripts", "--no-progress", "--registry", registry];
    if (refreshCache) args.push("--force", "--no-cache");
    return await runBun(args, installDir);
  } finally {
    await rm(installDir, { recursive: true, force: true });
  }
}

function runBun(args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(bunCommand, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      resolve({ exitCode: code ?? -1, signal, stdout, stderr });
    });
  });
}

function summarize(result) {
  const detail = result.stderr.trim() || result.stdout.trim() || `signal ${result.signal ?? "none"}`;
  return `exit ${result.exitCode}: ${truncate(detail, 800)}`;
}

function truncate(value, maxLength) {
  if (value.length <= maxLength) return value;
  return `${value.slice(0, maxLength)}...`;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
