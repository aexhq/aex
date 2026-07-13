#!/usr/bin/env bun

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const bunCommand = "bun" in process.versions ? process.execPath : "bun";

export async function main(argv = process.argv.slice(2)) {
  const [packageName, version] = argv;
  if (!packageName || !version) {
    console.error("Usage: wait-for-bun-install.mjs <package> <version>");
    return 2;
  }

  const registry = (process.env.NPM_REGISTRY_URL ?? "https://registry.npmjs.org").replace(/\/$/, "");
  const timeoutMs = Number(process.env.BUN_INSTALL_WAIT_TIMEOUT_MS ?? 10 * 60_000);
  const intervalMs = Number(process.env.BUN_INSTALL_WAIT_INTERVAL_MS ?? 10_000);
  const attemptTimeoutMs = Number(process.env.BUN_INSTALL_ATTEMPT_TIMEOUT_MS ?? 60_000);
  const deadline = Date.now() + timeoutMs;
  const spec = `${packageName}@${version}`;
  let attempt = 0;
  let lastStatus = "not checked";

  while (Date.now() < deadline) {
    attempt++;
    const defaultResult = await tryInstall(spec, registry, false, attemptTimeoutMs);
    if (defaultResult.exitCode === 0) {
      console.log(`${spec} is installable with bun.`);
      return 0;
    }

    const refreshResult = await tryInstall(spec, registry, true, attemptTimeoutMs);
    let retryClassification;
    if (refreshResult.exitCode === 0) {
      const confirmResult = await tryInstall(spec, registry, false, attemptTimeoutMs);
      if (confirmResult.exitCode === 0) {
        console.log(`${spec} is installable with bun.`);
        return 0;
      }
      retryClassification = "cache-propagation";
      lastStatus = `forced no-cache install succeeded, default install still failed: ${summarize(confirmResult)}`;
    } else {
      retryClassification = classifyRetryableInstallFailure({
        packageName,
        version,
        outputs: [defaultResult.stderr, defaultResult.stdout, refreshResult.stderr, refreshResult.stdout],
      });
      lastStatus = `default install failed: ${summarize(defaultResult)}; forced no-cache install failed: ${summarize(refreshResult)}`;
    }

    if (!retryClassification) {
      console.error(`bun install failed for ${spec} with a deterministic or unclassified error; not retrying. ${lastStatus}`);
      return 1;
    }

    console.warn(
      `bun install attempt ${attempt} for ${spec} failed with retryable class ${retryClassification}; retrying. Last status: ${lastStatus}`
    );
    await sleep(intervalMs);
  }

  console.error(`Timed out waiting for ${spec} to install with bun. Last status: ${lastStatus}`);
  return 1;
}

export function classifyRetryableInstallFailure({ packageName, version, outputs }) {
  const output = outputs.filter(Boolean).join("\n").toLowerCase();
  const targetMentioned = output.includes(packageName.toLowerCase()) && output.includes(version.toLowerCase());

  if (targetMentioned && /(?:\b404\b|not found|no version matching|could not find|failed to resolve)/u.test(output)) {
    return "target-version-not-visible";
  }
  if (/\b(?:eai_again|econnreset|etimedout|econnrefused|enetunreach|ehostunreach|und_err_[a-z_]+)\b|fetch failed|socket hang up|connection (?:reset|closed)|tls handshake/u.test(output)) {
    return "registry-transport";
  }
  if (/\b429\b|too many requests|rate limit/u.test(output)) {
    return "registry-rate-limit";
  }
  if (/\b(?:http|status(?: code)?)\s*(?:500|502|503|504)\b|service unavailable|bad gateway|gateway timeout/u.test(output)) {
    return "registry-server";
  }
  if (/(?:integrity|checksum).*(?:mismatch|failed)|(?:mismatch|failed).*(?:integrity|checksum)/u.test(output)) {
    return "integrity-propagation";
  }
  return null;
}

async function tryInstall(spec, registry, refreshCache, timeout) {
  const installDir = await mkdtemp(join(tmpdir(), "aex-bun-install-check-"));
  const cacheDir = await mkdtemp(join(tmpdir(), "aex-bun-install-cache-"));
  try {
    await writeFile(
      join(installDir, "package.json"),
      JSON.stringify({ name: "aex-bun-install-check", version: "0.0.0", private: true }, null, 2)
    );

    const args = [
      "install",
      spec,
      "--ignore-scripts",
      "--no-progress",
      "--registry",
      registry,
      "--cache-dir",
      cacheDir
    ];
    if (refreshCache) args.push("--force", "--no-cache");
    return await runBun(args, installDir, timeout);
  } finally {
    await rm(installDir, { recursive: true, force: true });
    await rm(cacheDir, { recursive: true, force: true });
  }
}

function runBun(args, cwd, timeout) {
  return new Promise((resolve, reject) => {
    const child = spawn(bunCommand, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      child.kill("SIGKILL");
      resolve({
        exitCode: -1,
        signal: "SIGKILL",
        stdout,
        stderr: `${stderr}\ntimed out after ${timeout}ms: bun ${args.join(" ")}`
      });
    }, timeout);
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
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

if (import.meta.main) {
  process.exit(await main());
}
