#!/usr/bin/env bun

import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { listJunitTestcases } from "./junit-report.mjs";

const SHA1 = /^[0-9a-f]{40}$/;
const SHA256 = /^sha256:[0-9a-f]{64}$/;
const POSITIVE_INTEGER = /^[1-9][0-9]*$/;
const REPOSITORY = "aexhq/aex";
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

export function assertExactCoordinates(value) {
  const repository = requiredString(value.repository, "repository");
  const sourceSha = requiredString(value.sourceSha, "source SHA");
  const releaseId = requiredString(value.releaseId, "release id");
  const deploymentContextDigest = requiredString(
    value.deploymentContextDigest,
    "deployment context digest"
  );
  const releaseTag = requiredString(value.releaseTag, "release tag");
  if (repository !== REPOSITORY) throw new Error(`release-evidence repository must be ${REPOSITORY}`);
  if (!SHA1.test(sourceSha)) throw new Error("release-evidence source SHA must be one exact lowercase commit SHA");
  if (!SHA256.test(releaseId)) throw new Error("release-evidence release id must be one exact sha256 digest");
  if (!SHA256.test(deploymentContextDigest)) {
    throw new Error("release-evidence deployment context digest must be one exact sha256 digest");
  }
  const tag = new RegExp(`^main-${sourceSha}-run-[1-9][0-9]*-attempt-[1-9][0-9]*$`);
  if (!tag.test(releaseTag)) throw new Error("release-evidence tag is not bound to the exact source SHA and main run");

  const base = `https://github.com/${repository}/releases/download/${releaseTag}`;
  assertBlob(value, "manifest", `${base}/composition-manifest.json`);
  assertBlob(value, "releaseTool", `${base}/aex-release-tool`);
  return true;
}

export function assertExactDeploymentHealth(value, expected) {
  if (!isRecord(value)) throw new Error("release-evidence health response must be one JSON object");
  const keys = Object.keys(value).sort();
  if (JSON.stringify(keys) !== JSON.stringify(["releaseId", "schema", "status"])) {
    throw new Error("release-evidence health response has fields outside the closed schema");
  }
  const releaseId = requiredString(expected.releaseId, "expected release id");
  if (!SHA256.test(releaseId)) throw new Error("release-evidence expected release id is not exact sha256");
  if (value.schema !== "aex.release-health.v1") {
    throw new Error("release-evidence health response has the wrong schema");
  }
  if (value.releaseId !== releaseId) {
    throw new Error("release-evidence health response names another release");
  }
  if (value.status !== "ready") {
    throw new Error("release-evidence health response is not ready");
  }
  return { schema: value.schema, releaseId: value.releaseId, status: value.status };
}

export async function checkExactDeploymentHealth(options) {
  const releaseId = requiredString(options.releaseId, "release id");
  const deploymentContextDigest = requiredString(
    options.deploymentContextDigest,
    "deployment context digest"
  );
  if (!SHA256.test(deploymentContextDigest)) {
    throw new Error("release-evidence deployment context digest must be one exact sha256 digest");
  }
  const phase = requiredString(options.phase, "deployment health phase");
  if (phase !== "before" && phase !== "after") {
    throw new Error("release-evidence deployment health phase must be before or after");
  }
  const url = exactHealthUrl(options.healthUrl, options.expectedHost);
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  const response = await fetchImpl(url, {
    headers: { accept: "application/json" },
    redirect: "error",
    signal: AbortSignal.timeout(20_000)
  });
  if (response.status !== 200) {
    throw new Error(`release-evidence exact deployment health returned HTTP ${response.status}`);
  }
  const contentType = String(response.headers.get("content-type") ?? "").toLowerCase();
  if (!/^application\/json(?:\s*;|$)/.test(contentType)) {
    throw new Error("release-evidence exact deployment health did not return application/json");
  }
  const cacheControl = String(response.headers.get("cache-control") ?? "").toLowerCase();
  if (!/(?:^|,)\s*no-store(?:\s*(?:,|$))/.test(cacheControl)) {
    throw new Error("release-evidence exact deployment health response is cacheable");
  }
  let parsed;
  try {
    parsed = JSON.parse(await response.text());
  } catch {
    throw new Error("release-evidence exact deployment health returned invalid JSON");
  }
  const health = assertExactDeploymentHealth(parsed, { releaseId });
  return {
    schema: "aex.release-health-observation.v1",
    phase,
    releaseId,
    deploymentContextDigest,
    responseDigest: digestJson(health),
    checkedAt: new Date().toISOString()
  };
}

export function assertRunnableScenarioMatrix(value) {
  if (!isRecord(value) || !Array.isArray(value.include) || value.include.length === 0) {
    throw new Error("release-evidence requires a non-empty runnable scenario matrix");
  }
  const names = new Set();
  for (const [index, entry] of value.include.entries()) {
    if (!isRecord(entry)) throw new Error(`release-evidence scenario ${index} is not an object`);
    const name = requiredString(entry.name, `scenario ${index} name`);
    const id = requiredString(entry.id, `scenario ${index} id`);
    const packageName = requiredString(entry.package, `scenario ${index} package`);
    requiredString(entry.target, `scenario ${index} target`);
    if (id !== `scenario:${name}` || !/^SC-[A-Z0-9-]+$/.test(name)) {
      throw new Error(`release-evidence scenario ${index} has an invalid exact identity`);
    }
    if (!/^(?:cargo:[a-z][a-z0-9-]*|npm:@aexhq\/[a-z][a-z0-9-]*)$/.test(packageName)) {
      throw new Error(`release-evidence scenario ${name} has an invalid package node`);
    }
    if (!Number.isInteger(entry.partition) || !Number.isInteger(entry.partitions) ||
      entry.partition < 0 || entry.partitions < 1 || entry.partition >= entry.partitions) {
      throw new Error(`release-evidence scenario ${name} has an invalid partition`);
    }
    if (names.has(name)) throw new Error(`release-evidence scenario ${name} is duplicated`);
    names.add(name);
  }
  return value.include;
}

export function assertUserJourneyInventory(listOutput, liveIds) {
  const allowed = new Set(liveIds.filter((id) => /^live\.[a-z0-9]+(?:-[a-z0-9]+)*$/.test(id)));
  const selected = [...allowed].filter((id) => new RegExp(`\\(pass\\)\\s+${escapeRegex(id)}(?:\\s|\\[|$)`).test(listOutput));
  if (selected.length === 0) {
    throw new Error("release-evidence requires a non-empty runnable user journey inventory; registry-only tests are not evidence");
  }
  return selected.sort();
}

export function validateHygieneReport(report, expected) {
  if (!isRecord(report) || report.schema !== "aex.release-evidence-hygiene.v1") {
    throw new Error("release-evidence hygiene report has the wrong schema");
  }
  for (const [field, wanted] of [
    ["suite", expected.suite],
    ["releaseId", expected.releaseId],
    ["workflowRunId", expected.workflowRunId],
    ["secretCanaryDigest", expected.secretCanaryDigest]
  ]) {
    if (report[field] !== wanted) throw new Error(`release-evidence hygiene ${field} is not bound to this run`);
  }
  if (!SHA256.test(String(report.cleanupLedgerDigest ?? ""))) {
    throw new Error("release-evidence hygiene has no exact cleanup-ledger digest");
  }
  if (!Array.isArray(report.provisioned) || !Array.isArray(report.residue)) {
    throw new Error("release-evidence hygiene inventory is missing");
  }
  if (report.residue.length !== 0) throw new Error("release-evidence cleanup left residue");
  for (const resource of report.provisioned) {
    if (!isRecord(resource) || !requiredString(resource.kind, "provisioned resource kind") ||
      !requiredString(resource.id, "provisioned resource id") || resource.reclaimed !== true) {
      throw new Error("release-evidence cleanup did not reclaim every provisioned resource");
    }
  }
  const budget = exactNonNegativeInteger(report.budgetMicroUsd, "budgetMicroUsd");
  const spent = exactNonNegativeInteger(report.spentMicroUsd, "spentMicroUsd");
  const maximum = exactNonNegativeInteger(expected.maximumBudgetMicroUsd, "maximumBudgetMicroUsd");
  if (budget > maximum) throw new Error("release-evidence declared spend budget exceeds the protected maximum");
  if (spent > budget) throw new Error("release-evidence observed spend exceeds its declared budget");
  if (report.secretCanaryObserved !== false) throw new Error("release-evidence secret canary was observed outside its boundary");

  return {
    budgetMicroUsd: budget,
    spentMicroUsd: spent,
    cleanupLedgerDigest: report.cleanupLedgerDigest,
    residue: "none",
    secretCanaryObserved: false
  };
}

async function main(argv) {
  const [command, ...rest] = argv;
  const options = parseOptions(rest);
  switch (command) {
    case "coordinates":
      assertExactCoordinates({
        repository: options.repository,
        sourceSha: options["source-sha"],
        releaseId: options["release-id"],
        deploymentContextDigest: options["deployment-context-digest"],
        releaseTag: options["release-tag"],
        manifestUri: options["manifest-uri"],
        manifestDigest: options["manifest-digest"],
        manifestSizeBytes: options["manifest-size-bytes"],
        releaseToolUri: options["release-tool-uri"],
        releaseToolDigest: options["release-tool-digest"],
        releaseToolSizeBytes: options["release-tool-size-bytes"]
      });
      break;
    case "deployment-health": {
      const observation = await checkExactDeploymentHealth({
        healthUrl: requiredOption(options, "health-url"),
        expectedHost: requiredOption(options, "expected-host"),
        releaseId: requiredOption(options, "release-id"),
        deploymentContextDigest: requiredOption(options, "deployment-context-digest"),
        phase: requiredOption(options, "phase")
      });
      writeJson(requiredOption(options, "out"), observation);
      break;
    }
    case "matrix":
      assertRunnableScenarioMatrix(readJson(requiredOption(options, "file")));
      break;
    case "user-inventory": {
      const listed = readFileSync(requiredOption(options, "list"), "utf8");
      const liveIds = readJson(requiredOption(options, "live-ids"));
      if (!Array.isArray(liveIds)) throw new Error("release-evidence live ids must be a JSON array");
      const journeys = assertUserJourneyInventory(listed, liveIds);
      writeJson(requiredOption(options, "out"), { schema: "aex.release-evidence-inventory.v1", journeys });
      break;
    }
    case "run-e2e":
      runE2e(readJson(requiredOption(options, "matrix")), requiredOption(options, "output-root"));
      break;
    case "run-user":
      await runUser(requiredOption(options, "output-root"));
      break;
    case "validate-hygiene": {
      const data = validateHygieneReport(readJson(requiredOption(options, "file")), {
        suite: requiredOption(options, "suite"),
        releaseId: requiredOption(options, "release-id"),
        workflowRunId: requiredOption(options, "workflow-run-id"),
        secretCanaryDigest: requiredOption(options, "secret-canary-digest"),
        maximumBudgetMicroUsd: requiredOption(options, "maximum-budget-micro-usd")
      });
      writeJson(requiredOption(options, "out"), data);
      break;
    }
    default:
      throw new Error("usage: release-evidence.mjs <coordinates|deployment-health|matrix|user-inventory|run-e2e|run-user|validate-hygiene> [options]");
  }
}

function runE2e(matrix, outputRoot) {
  const entries = assertRunnableScenarioMatrix(matrix);
  const absoluteRoot = resolveInsideRepository(outputRoot);
  mkdirSync(absoluteRoot, { recursive: true });
  const reports = [];
  const inventories = [];
  const hygiene = [];
  for (const [index, entry] of entries.entries()) {
    const scenarioRoot = resolve(absoluteRoot, `${String(index).padStart(3, "0")}-${entry.name}`);
    mkdirSync(scenarioRoot, { recursive: true });
    const junit = resolve(scenarioRoot, "junit.xml");
    const inventory = resolve(scenarioRoot, "inventory.json");
    const hygienePath = resolve(scenarioRoot, "hygiene.json");
    const env = {
      ...process.env,
      AEX_RELEASE_EVIDENCE_SCENARIO: entry.name,
      AEX_RELEASE_EVIDENCE_HYGIENE_PATH: hygienePath
    };
    if (entry.package.startsWith("cargo:")) {
      const packageName = entry.package.slice("cargo:".length);
      const listed = run(["cargo", "nextest", "list", "--locked", "--profile", "live", "-p", packageName,
        "--test", entry.target, "--message-format", "json"], { env, capture: true });
      writeFileSync(resolve(scenarioRoot, "nextest-list.json"), listed);
      const cases = nextestCases(listed);
      if (cases.length === 0) throw new Error(`release-evidence scenario ${entry.name} declared no Cargo cases`);
      writeJson(inventory, { schema: "aex.release-evidence-inventory.v1", scenario: entry.name, cases });
      const nextestJunit = resolve(repoRoot, "target", "nextest", "live", "junit.xml");
      rmSync(nextestJunit, { force: true });
      run(["cargo", "nextest", "run", "--locked", "--profile", "live", "-p", packageName,
        "--test", entry.target, "--no-tests=fail"], { env });
      if (!existsSync(nextestJunit)) throw new Error(`release-evidence scenario ${entry.name} emitted no JUnit report`);
      copyFileSync(nextestJunit, junit);
    } else {
      const packageName = entry.package.slice("npm:".length);
      const npmEnv = { ...env, AEX_RELEASE_EVIDENCE_INVENTORY_PATH: inventory,
        AEX_RELEASE_EVIDENCE_JUNIT_PATH: junit, AEX_RELEASE_EVIDENCE_MODE: "inventory" };
      run(["bun", "--filter", packageName, "run", `test:${entry.target}`], { env: npmEnv });
      if (!existsSync(inventory)) {
        throw new Error(`release-evidence scenario ${entry.name} emitted no pre-run inventory`);
      }
      npmEnv.AEX_RELEASE_EVIDENCE_MODE = "execute";
      run(["bun", "--filter", packageName, "run", `test:${entry.target}`], { env: npmEnv });
      if (!existsSync(junit)) throw new Error(`release-evidence scenario ${entry.name} emitted no JUnit report`);
    }
    assertClosedJunit(junit, readJson(inventory));
    if (!existsSync(hygienePath)) throw new Error(`release-evidence scenario ${entry.name} emitted no hygiene report`);
    reports.push(junit);
    inventories.push(readJson(inventory));
    hygiene.push(readJson(hygienePath));
  }
  writeMergedJunit(reports, resolve(absoluteRoot, "junit.xml"));
  writeJson(resolve(absoluteRoot, "inventory.json"), {
    schema: "aex.release-evidence-inventory.v1",
    suite: "e2e",
    cases: inventories.flatMap((entry) => entry.cases ?? [])
  });
  writeJson(resolve(absoluteRoot, "hygiene.json"), mergeHygiene(hygiene, "e2e"));
}

async function runUser(outputRoot) {
  const absoluteRoot = resolveInsideRepository(outputRoot);
  mkdirSync(absoluteRoot, { recursive: true });
  const listPath = resolve(absoluteRoot, "list.txt");
  const junit = resolve(absoluteRoot, "junit.xml");
  const hygienePath = resolve(absoluteRoot, "hygiene.json");
  const listed = run(["bun", "test", "apps/user-tests/test/live"], {
    capture: true,
    env: { ...process.env, AEX_RELEASE_EVIDENCE_MODE: "inventory" }
  });
  writeFileSync(listPath, listed);
  const scenarios = await import(new URL("../../apps/user-tests/scenarios.ts", import.meta.url));
  const liveIds = scenarios.USER_SCENARIOS.filter(({ suite }) => suite === "live").map(({ id }) => id);
  const journeys = assertUserJourneyInventory(listed, liveIds);
  const cases = [...listed.matchAll(/^\(pass\)\s+(.+?)(?:\s+\[[^\]]+\])?$/gm)].map((match) => match[1].trim());
  if (cases.length === 0) throw new Error("release-evidence user suite declared no cases");
  writeJson(resolve(absoluteRoot, "inventory.json"), {
    schema: "aex.release-evidence-inventory.v1", suite: "user", cases, journeys
  });
  run(["bun", "test", "--isolate", "apps/user-tests/test/live", "--reporter=junit",
    `--reporter-outfile=${junit}`], {
    env: {
      ...process.env,
      AEX_RELEASE_EVIDENCE_HYGIENE_PATH: hygienePath,
      AEX_RELEASE_EVIDENCE_MODE: "execute"
    }
  });
  assertClosedJunit(junit, { cases });
  if (!existsSync(hygienePath)) throw new Error("release-evidence user suite emitted no hygiene report");
}

function assertClosedJunit(path, inventory) {
  if (!isRecord(inventory) || !Array.isArray(inventory.cases) || inventory.cases.length === 0) {
    throw new Error(`release-evidence inventory for ${path} is empty`);
  }
  const cases = listJunitTestcases(readFileSync(path, "utf8"));
  if (cases.length !== inventory.cases.length) {
    throw new Error(`release-evidence JUnit collected ${cases.length} case(s), not the ${inventory.cases.length} declared`);
  }
  if (cases.some(({ status }) => status !== "passed")) {
    throw new Error("release-evidence JUnit contains a failed, skipped or todo case");
  }
}

function nextestCases(text) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw new Error(`release-evidence nextest inventory is not one closed JSON document: ${error.message}`);
  }
  const cases = [];
  const visit = (value, key = "") => {
    if (Array.isArray(value)) return value.forEach((entry) => visit(entry, key));
    if (!isRecord(value)) return;
    if (key === "testcases") cases.push(...Object.keys(value));
    for (const [childKey, child] of Object.entries(value)) visit(child, childKey);
  };
  visit(parsed);
  return [...new Set(cases)].sort();
}

function writeMergedJunit(paths, out) {
  const cases = paths.flatMap((path) => listJunitTestcases(readFileSync(path, "utf8")));
  if (cases.length === 0) throw new Error("release-evidence cannot merge an empty JUnit set");
  const body = cases.map((testcase) => {
    const attributes = `name="${xml(testcase.fullName)}" file="${xml(testcase.file)}"`;
    if (testcase.status === "passed") return `    <testcase ${attributes} />`;
    if (testcase.status === "failed") return `    <testcase ${attributes}><failure>${xml(testcase.message)}</failure></testcase>`;
    return `    <testcase ${attributes}><skipped /></testcase>`;
  }).join("\n");
  writeFileSync(out, `<?xml version="1.0" encoding="UTF-8"?>\n<testsuites tests="${cases.length}">\n  <testsuite name="release-e2e" tests="${cases.length}">\n${body}\n  </testsuite>\n</testsuites>\n`);
}

function mergeHygiene(reports, suite) {
  if (reports.length === 0) throw new Error("release-evidence has no hygiene reports to merge");
  const first = reports[0];
  for (const report of reports) {
    if (!isRecord(report) || report.schema !== "aex.release-evidence-hygiene.v1" || report.suite !== suite) {
      throw new Error("release-evidence scenario emitted an invalid hygiene report");
    }
    for (const field of ["schema", "releaseId", "workflowRunId", "secretCanaryDigest"]) {
      if (report[field] !== first[field]) throw new Error(`release-evidence hygiene ${field} differs across scenarios`);
    }
    if (!SHA256.test(String(report.cleanupLedgerDigest ?? ""))) {
      throw new Error("release-evidence scenario emitted no exact cleanup-ledger digest");
    }
  }
  const provisioned = reports.flatMap((report) => Array.isArray(report.provisioned) ? report.provisioned : []);
  const residue = reports.flatMap((report) => Array.isArray(report.residue) ? report.residue : ["invalid-hygiene"]);
  const cleanupLedgerDigest = digestJson(reports.map((report) => report.cleanupLedgerDigest));
  return {
    schema: "aex.release-evidence-hygiene.v1",
    suite,
    releaseId: first.releaseId,
    workflowRunId: first.workflowRunId,
    budgetMicroUsd: reports.reduce((total, report) => total + exactNonNegativeInteger(report.budgetMicroUsd, "budgetMicroUsd"), 0),
    spentMicroUsd: reports.reduce((total, report) => total + exactNonNegativeInteger(report.spentMicroUsd, "spentMicroUsd"), 0),
    cleanupLedgerDigest,
    provisioned,
    residue,
    secretCanaryDigest: first.secretCanaryDigest,
    secretCanaryObserved: reports.some((report) => report.secretCanaryObserved !== false)
  };
}

function run(argv, options = {}) {
  const result = spawnSync(argv[0], argv.slice(1), {
    cwd: repoRoot,
    env: options.env ?? process.env,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit"
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    if (options.capture && result.stderr) process.stderr.write(result.stderr);
    throw new Error(`release-evidence command failed (${result.status}): ${argv.join(" ")}`);
  }
  return options.capture ? result.stdout : "";
}

function assertBlob(value, prefix, expectedUri) {
  if (requiredString(value[`${prefix}Uri`], `${prefix} URI`) !== expectedUri) {
    throw new Error(`release-evidence ${prefix} URI is outside the exact main release`);
  }
  if (!SHA256.test(requiredString(value[`${prefix}Digest`], `${prefix} digest`))) {
    throw new Error(`release-evidence ${prefix} digest must be exact sha256`);
  }
  if (!POSITIVE_INTEGER.test(String(value[`${prefix}SizeBytes`] ?? ""))) {
    throw new Error(`release-evidence ${prefix} size must be a positive integer`);
  }
}

function parseOptions(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith("--") || value === undefined) throw new Error(`invalid release-evidence option near ${flag ?? "(end)"}`);
    options[flag.slice(2)] = value;
  }
  return options;
}

function requiredOption(options, name) {
  return requiredString(options[name], `--${name}`);
}

function requiredString(value, label) {
  if (typeof value !== "string" || value.trim() === "") throw new Error(`release-evidence ${label} is required`);
  return value;
}

function exactHealthUrl(value, expectedHostValue) {
  const expectedHost = requiredString(expectedHostValue, "expected health host");
  let parsed;
  try {
    parsed = new URL(requiredString(value, "health URL"));
  } catch {
    throw new Error("release-evidence health URL must be absolute");
  }
  if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error("release-evidence health URL must be credential-free exact HTTPS without query or fragment");
  }
  if (parsed.hostname !== expectedHost) {
    throw new Error("release-evidence health URL host differs from its protected binding");
  }
  return parsed;
}

function exactNonNegativeInteger(value, label) {
  const parsed = typeof value === "string" && /^[0-9]+$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(parsed) || parsed < 0) throw new Error(`release-evidence ${label} must be a non-negative integer`);
  return parsed;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function resolveInsideRepository(path) {
  const absolute = resolve(repoRoot, path);
  const rel = relative(repoRoot, absolute);
  if (rel === ".." || rel.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(rel)) {
    throw new Error("release-evidence output root escaped the repository");
  }
  return absolute;
}

function digestJson(value) {
  return `sha256:${createHash("sha256").update(JSON.stringify(value)).digest("hex")}`;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function xml(value) {
  return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;").replaceAll("'", "&apos;");
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
