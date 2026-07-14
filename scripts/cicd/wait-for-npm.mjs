#!/usr/bin/env node

import { appendFile } from "node:fs/promises";

const DEFAULT_REGISTRY = "https://registry.npmjs.org";
const INTEGRITY_PATTERN = /^sha512-[A-Za-z0-9+/]+={0,2}$/u;
const SOURCE_SHA_PATTERN = /^[0-9a-f]{40}$/u;

export async function main(argv = process.argv.slice(2), dependencies = {}) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(
      "Usage: wait-for-npm.mjs <package> <version> --source-sha <sha> [--github-output <path>]"
    );
    return 2;
  }

  try {
    const evidence = await waitForNpmEvidence(options, dependencies);
    if (options.githubOutput) {
      const append = dependencies.appendFile ?? appendFile;
      await append(options.githubOutput, `integrity=${evidence.integrity}\n`, "utf8");
    }
    console.log(
      `${options.packageName}@${options.version} immutable npm evidence is visible ` +
        `(attempt ${evidence.attempt}, source ${options.sourceSha}, metadata HTTP 200, ` +
        `tarball HTTP ${evidence.tarballStatus}).`
    );
    return 0;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

export async function waitForNpmEvidence(options, dependencies = {}) {
  const fetchImpl = dependencies.fetch ?? fetch;
  const now = dependencies.now ?? Date.now;
  const sleepImpl = dependencies.sleep ?? sleep;
  const registry = (options.registry ?? process.env.NPM_REGISTRY_URL ?? DEFAULT_REGISTRY).replace(
    /\/$/u,
    ""
  );
  const timeoutMs = numberFromEnv("NPM_WAIT_TIMEOUT_MS", 10 * 60_000);
  const intervalMs = numberFromEnv("NPM_WAIT_INTERVAL_MS", 10_000);
  const metadataUrl = `${registry}/${encodeURIComponent(options.packageName)}/${encodeURIComponent(options.version)}`;
  const deadline = now() + timeoutMs;
  let attempt = 0;
  let lastStatus = "not checked";

  while (now() < deadline) {
    attempt += 1;
    let metadata;
    try {
      metadata = await fetchImpl(metadataUrl, {
        headers: {
          accept: "application/json",
          "cache-control": "no-cache"
        }
      });
    } catch (error) {
      lastStatus = `metadata transport error: ${error instanceof Error ? error.message : String(error)}`;
      console.warn(`npm evidence attempt ${attempt}: ${lastStatus}; retrying.`);
      await sleepImpl(intervalMs);
      continue;
    }

    if (!metadata.ok) {
      lastStatus = `metadata HTTP ${metadata.status}`;
      if (!isRetryableRegistryStatus(metadata.status)) {
        throw new Error(
          `npm evidence failed for ${options.packageName}@${options.version}: ${lastStatus} is not retryable.`
        );
      }
      console.warn(`npm evidence attempt ${attempt}: ${lastStatus}; retrying.`);
      await sleepImpl(intervalMs);
      continue;
    }

    let body;
    try {
      body = await metadata.json();
    } catch (error) {
      throw new Error(
        `npm evidence failed for ${options.packageName}@${options.version}: metadata HTTP 200 returned invalid JSON ` +
          `(${error instanceof Error ? error.message : String(error)}).`
      );
    }

    const evidence = validateRegistryMetadata(body, options);
    let tarball;
    try {
      tarball = await checkTarball(evidence.tarballUrl, fetchImpl);
    } catch (error) {
      lastStatus = `tarball transport error: ${error instanceof Error ? error.message : String(error)}`;
      console.warn(`npm evidence attempt ${attempt}: ${lastStatus}; retrying.`);
      await sleepImpl(intervalMs);
      continue;
    }
    lastStatus = `metadata HTTP 200; tarball HTTP ${tarball.status}`;
    if (tarball.visible) {
      return { ...evidence, attempt, tarballStatus: tarball.status };
    }
    if (!isRetryableRegistryStatus(tarball.status)) {
      throw new Error(
        `npm evidence failed for ${options.packageName}@${options.version}: ${lastStatus} is not retryable.`
      );
    }
    console.warn(`npm evidence attempt ${attempt}: ${lastStatus}; retrying.`);
    await sleepImpl(intervalMs);
  }

  throw new Error(
    `Timed out waiting for immutable npm evidence for ${options.packageName}@${options.version}. ` +
      `Attempts: ${attempt}. Last status: ${lastStatus}.`
  );
}

export function validateRegistryMetadata(body, expected) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw new Error("npm registry metadata must be a JSON object.");
  }
  if (body.name !== expected.packageName) {
    throw new Error(
      `npm registry package identity '${String(body.name ?? "missing")}' does not match '${expected.packageName}'.`
    );
  }
  if (body.version !== expected.version) {
    throw new Error(
      `npm registry version identity '${String(body.version ?? "missing")}' does not match '${expected.version}'.`
    );
  }

  const integrity = body.dist?.integrity;
  if (typeof integrity !== "string" || !INTEGRITY_PATTERN.test(integrity)) {
    throw new Error(`npm registry returned invalid dist.integrity for ${expected.packageName}@${expected.version}.`);
  }
  const tarballUrl = body.dist?.tarball;
  if (typeof tarballUrl !== "string" || !/^https:\/\//u.test(tarballUrl)) {
    throw new Error(`npm registry returned invalid dist.tarball for ${expected.packageName}@${expected.version}.`);
  }

  const sourceSha = body.aexRelease?.sourceSha;
  if (sourceSha !== expected.sourceSha) {
    throw new Error(
      `npm registry source attestation '${String(sourceSha ?? "missing")}' does not match release source ` +
        `'${expected.sourceSha}'.`
    );
  }
  return { integrity, sourceSha, tarballUrl };
}

export function isRetryableRegistryStatus(status) {
  return status === 404 || status === 408 || status === 425 || status === 429 || status >= 500;
}

async function checkTarball(url, fetchImpl) {
  const head = await fetchImpl(url, {
    method: "HEAD",
    headers: { "cache-control": "no-cache" }
  });
  if (head.ok) return { visible: true, status: head.status };
  if (head.status !== 405 && head.status !== 403) {
    return { visible: false, status: head.status };
  }

  const get = await fetchImpl(url, {
    headers: {
      "cache-control": "no-cache",
      range: "bytes=0-0"
    }
  });
  return { visible: get.ok || get.status === 206, status: get.status };
}

function parseArgs(argv) {
  const [packageName, version, ...rest] = argv;
  if (!packageName || !version) throw new Error("Package name and version are required.");

  const options = { packageName, version };
  for (let index = 0; index < rest.length; index += 1) {
    const flag = rest[index];
    const value = rest[index + 1];
    if ((flag === "--source-sha" || flag === "--github-output") && value) {
      if (flag === "--source-sha") options.sourceSha = value;
      if (flag === "--github-output") options.githubOutput = value;
      index += 1;
      continue;
    }
    throw new Error(`Unknown or incomplete option: ${flag}`);
  }
  if (!SOURCE_SHA_PATTERN.test(options.sourceSha ?? "")) {
    throw new Error("--source-sha must be a full lowercase 40-character commit SHA.");
  }
  return options;
}

function numberFromEnv(name, fallback) {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number.`);
  return value;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

if (import.meta.main) {
  process.exitCode = await main();
}
