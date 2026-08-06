#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  lstatSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync
} from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCHEMA = "aex.model-catalog-qualification-binding.v1";
const TARGET_PROVIDER = "deepseek";
const TARGET_MODEL = "deepseek-v4-flash";
const QUALIFIER_WORKFLOW = ".github/workflows/model-catalog-qualify.yml";
const SOURCE_REF = "refs/heads/main";
const FILES = Object.freeze({
  evidence: "evidence.json",
  receipt: "receipt.json"
});
const BINDING_FILE = "qualification-binding.json";
const TOKENIZER_SHA256 = "sha256:ecb6f9fc369894346f0511f4074ca75cee5cd5f3b06d02f1ba35fcd39f8e121d";
const CHAT_TEMPLATE_SHA256 = "sha256:3a3036eeb96ca0e48565fc1f19b4d7d3fa17ada32bb1b3d5ed539ae74cf22923";
const PROBES = Object.freeze(Array.from({ length: 23 }, (_, index) => `p-${String(index + 1).padStart(2, "0")}`));

function fail(message) {
  throw new Error(message);
}

function canonicalValue(value) {
  if (Array.isArray(value)) {
    return value.map(canonicalValue);
  }
  if (value !== null && typeof value === "object") {
    const result = {};
    for (const key of Object.keys(value).sort()) {
      result[key] = canonicalValue(value[key]);
    }
    return result;
  }
  return value;
}

export function canonicalJsonBytes(value) {
  return Buffer.from(JSON.stringify(canonicalValue(value)), "utf8");
}

function sha256(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function expectPattern(value, pattern, name) {
  if (typeof value !== "string" || !pattern.test(value)) {
    fail(`${name} is invalid`);
  }
  return value;
}

function positiveInteger(value, name) {
  if (typeof value !== "string" || !/^[1-9][0-9]*$/.test(value)) {
    fail(`${name} must be a positive base-10 integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    fail(`${name} exceeds the safe integer range`);
  }
  return parsed;
}

function positiveJsonInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${name} must be a positive JSON integer`);
  }
  return value;
}

function nonnegativeJsonInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) {
    fail(`${name} must be a non-negative JSON integer`);
  }
  return value;
}

function exactKeys(value, keys, name) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${name} must be a JSON object`);
  }
  const observed = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (JSON.stringify(observed) !== JSON.stringify(expected)) {
    fail(`${name} has fields outside its closed schema`);
  }
  return value;
}

function sameJson(left, right) {
  return canonicalJsonBytes(left).equals(canonicalJsonBytes(right));
}

function timestamp(value, name) {
  return expectPattern(
    value,
    /^[0-9]{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12][0-9]|3[01])T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]\.[0-9]{3}Z$/,
    name
  );
}

function validateEvidence(evidence, receipt, expected) {
  const value = exactKeys(evidence.value, [
    "aggregate_usage",
    "authority",
    "bounds",
    "probes",
    "run",
    "schema",
    "source",
    "target"
  ], "qualification evidence");
  if (value.schema !== "aex.model-catalog-qualification-evidence.v1") {
    fail("qualification evidence schema is invalid");
  }
  const target = exactKeys(value.target, ["model", "provider"], "evidence target");
  if (target.provider !== TARGET_PROVIDER || target.model !== TARGET_MODEL) {
    fail("qualification evidence target is invalid");
  }
  const source = exactKeys(value.source, ["repository", "sha"], "evidence source");
  if (source.repository !== expected.repository || source.sha !== expected.sourceSha) {
    fail("qualification evidence source does not equal the protected workflow source");
  }
  const run = exactKeys(value.run, ["attempt", "id", "issued_at", "plane", "ran_at", "region"], "evidence run");
  if (
    run.id !== expected.runId ||
    run.attempt !== expected.runAttempt ||
    run.plane !== "dev" ||
    run.region !== "eu-west-1"
  ) {
    fail("qualification evidence run identity is invalid");
  }
  const ranAt = timestamp(run.ran_at, "evidence ran_at");
  const issuedAt = timestamp(run.issued_at, "evidence issued_at");
  if (issuedAt < ranAt) {
    fail("qualification evidence issued_at precedes ran_at");
  }
  const authority = exactKeys(
    value.authority,
    ["chat_template_sha256", "publisher", "tokenizer_sha256"],
    "evidence authority"
  );
  if (
    authority.publisher !== "aex-catalog-prd" ||
    authority.tokenizer_sha256 !== TOKENIZER_SHA256 ||
    authority.chat_template_sha256 !== CHAT_TEMPLATE_SHA256
  ) {
    fail("qualification evidence authority identities are invalid");
  }
  const bounds = exactKeys(
    value.bounds,
    ["approved_budget_micro_usd", "approved_runtime_seconds", "maximum_cost_micro_usd"],
    "evidence bounds"
  );
  if (
    bounds.approved_budget_micro_usd !== expected.maximumBudgetMicroUsd ||
    bounds.approved_runtime_seconds !== expected.maximumRuntimeSeconds ||
    positiveJsonInteger(bounds.maximum_cost_micro_usd, "evidence maximum_cost_micro_usd") >
      bounds.approved_budget_micro_usd
  ) {
    fail("qualification evidence bounds do not equal the protected workflow bounds");
  }

  const usage = exactKeys(value.aggregate_usage, [
    "cache_read_input_tokens",
    "cache_write_input_tokens",
    "completeness",
    "input_tokens",
    "output_tokens",
    "provider_total_tokens",
    "reasoning_tokens",
    "tool_use_prompt_tokens"
  ], "evidence aggregate_usage");
  for (const field of [
    "cache_read_input_tokens",
    "cache_write_input_tokens",
    "input_tokens",
    "output_tokens",
    "reasoning_tokens",
    "tool_use_prompt_tokens"
  ]) {
    nonnegativeJsonInteger(usage[field], `evidence aggregate_usage.${field}`);
  }
  const completeness = exactKeys(usage.completeness, ["kind"], "evidence usage completeness");
  if (
    completeness.kind !== "absent" ||
    usage.provider_total_tokens !== null ||
    usage.reasoning_tokens > usage.output_tokens
  ) {
    fail("qualification evidence aggregate usage does not match the incomplete matrix contract");
  }

  if (!Array.isArray(value.probes) || value.probes.length !== PROBES.length) {
    fail("qualification evidence must contain the complete 23-probe inventory");
  }
  const observedProbes = value.probes.map((probe, index) => {
    const row = exactKeys(probe, ["duration_ms", "observed", "outcome", "probe"], `evidence probe ${index}`);
    nonnegativeJsonInteger(row.duration_ms, `evidence probe ${index} duration_ms`);
    if (!Array.isArray(row.observed)) {
      fail(`evidence probe ${index} observed facts must be an array`);
    }
    for (const [factIndex, fact] of row.observed.entries()) {
      const observed = exactKeys(fact, ["key", "value"], `evidence probe ${index} fact ${factIndex}`);
      if (typeof observed.key !== "string" || typeof observed.value !== "string") {
        fail(`evidence probe ${index} fact ${factIndex} must contain strings`);
      }
    }
    const outcome = exactKeys(
      row.outcome,
      row.outcome?.outcome === "pass" ? ["outcome"] : ["capability", "outcome"],
      `evidence probe ${index} outcome`
    );
    if (outcome.outcome !== "pass" && outcome.outcome !== "not_applicable") {
      fail(`evidence probe ${index} outcome is invalid`);
    }
    return row.probe;
  });
  if (JSON.stringify(observedProbes) !== JSON.stringify(PROBES)) {
    fail("qualification evidence probe inventory is not exact and ordered");
  }

  if (
    receipt.value.ran_at !== ranAt ||
    receipt.value.plane !== "dev" ||
    receipt.value.region !== "eu-west-1" ||
    !sameJson(receipt.value.tokens_spent, usage) ||
    !Array.isArray(receipt.value.provider_request_ids) ||
    receipt.value.provider_request_ids.length !== 0 ||
    !Array.isArray(receipt.value.results) ||
    JSON.stringify(receipt.value.results.map((result) => result.probe)) !== JSON.stringify(PROBES)
  ) {
    fail("qualification receipt does not equal the evidence run, usage, and probe inventory");
  }
}

function readCanonicalJson(path, name) {
  const bytes = readFileSync(path);
  let value;
  try {
    value = JSON.parse(bytes.toString("utf8"));
  } catch {
    fail(`${name} is not valid UTF-8 JSON`);
  }
  if (!bytes.equals(canonicalJsonBytes(value))) {
    fail(`${name} is not canonical JSON`);
  }
  return { bytes, value };
}

function exactRegularFiles(directory, expected) {
  const root = realpathSync(directory);
  const observed = readdirSync(root).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(observed) !== JSON.stringify(wanted)) {
    fail(`qualification directory allowlist mismatch: expected ${wanted.join(", ")}; observed ${observed.join(", ")}`);
  }
  for (const name of wanted) {
    const path = join(root, name);
    const metadata = lstatSync(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      fail(`${name} is not a regular non-symlink file`);
    }
    if (realpathSync(path) !== path) {
      fail(`${name} resolves outside the qualification directory`);
    }
  }
  return root;
}

function qualificationRelationships(evidence, receipt) {
  if (receipt.value.evidence_digest !== sha256(evidence.bytes)) {
    fail("receipt evidence_digest does not identify evidence.json");
  }
  if (receipt.value.probe_suite_revision !== 1) {
    fail("receipt probe_suite_revision must be the launch revision 1");
  }
  expectPattern(receipt.value.catalog_entry_digest, /^sha256:[0-9a-f]{64}$/, "receipt catalog_entry_digest");
  return {
    adapter_source: expectPattern(receipt.value.adapter_source, /^blake3:[0-9a-f]{64}$/, "receipt adapter_source"),
    catalog_entry_digest: receipt.value.catalog_entry_digest,
    probe_suite_revision: receipt.value.probe_suite_revision
  };
}

function fileIdentity(path, bytes) {
  return Object.freeze({ path, sha256: sha256(bytes), size_bytes: bytes.length });
}

function parseArguments(argv) {
  const [command, ...tail] = argv;
  if (command !== "create" && command !== "verify") {
    fail("usage: model-catalog-qualification-binding.mjs <create|verify> --name value ...");
  }
  const options = {};
  for (let index = 0; index < tail.length; index += 2) {
    const flag = tail[index];
    const value = tail[index + 1];
    if (value === undefined || !flag?.startsWith("--")) {
      fail(`invalid argument near ${flag ?? "end of command"}`);
    }
    const name = flag.slice(2).replaceAll("-", "_");
    if (options[name] !== undefined) {
      fail(`${flag} was supplied more than once`);
    }
    options[name] = value;
  }
  return { command, options };
}

function required(options, name) {
  const value = options[name];
  if (typeof value !== "string" || value.length === 0) {
    fail(`--${name.replaceAll("_", "-")} is required`);
  }
  return value;
}

export function createBinding(options) {
  const directory = resolve(required(options, "directory"));
  const root = exactRegularFiles(directory, Object.values(FILES));
  const evidence = readCanonicalJson(join(root, FILES.evidence), FILES.evidence);
  const receipt = readCanonicalJson(join(root, FILES.receipt), FILES.receipt);
  const authority = qualificationRelationships(evidence, receipt);
  const repository = expectPattern(required(options, "repository"), /^aexhq\/aex$/, "repository");
  const sourceSha = expectPattern(required(options, "source_sha"), /^[0-9a-f]{40}$/, "source_sha");
  const sourceRef = expectPattern(required(options, "source_ref"), /^refs\/heads\/main$/, "source_ref");
  const workflowPath = expectPattern(required(options, "workflow_path"), /^\.github\/workflows\/model-catalog-qualify\.yml$/, "workflow_path");
  const workflowRef = required(options, "workflow_ref");
  if (workflowRef !== `${repository}/${workflowPath}@${sourceRef}`) {
    fail("workflow_ref does not identify the exact main qualification workflow");
  }
  const workflowSha = expectPattern(required(options, "workflow_sha"), /^[0-9a-f]{40}$/, "workflow_sha");
  if (workflowSha !== sourceSha) {
    fail("workflow_sha must equal the qualified exact-main source_sha");
  }
  const runId = positiveInteger(required(options, "run_id"), "run_id");
  const runAttempt = positiveInteger(required(options, "run_attempt"), "run_attempt");
  const maximumBudgetMicroUsd = positiveInteger(
    required(options, "maximum_budget_micro_usd"),
    "maximum_budget_micro_usd"
  );
  const maximumRuntimeSeconds = positiveInteger(
    required(options, "maximum_runtime_seconds"),
    "maximum_runtime_seconds"
  );
  validateEvidence(evidence, receipt, {
    repository,
    sourceSha,
    runId,
    runAttempt,
    maximumBudgetMicroUsd,
    maximumRuntimeSeconds
  });

  const binding = {
    schema: SCHEMA,
    target: { provider: TARGET_PROVIDER, model: TARGET_MODEL },
    qualification: {
      repository,
      source_sha: sourceSha,
      source_ref: sourceRef,
      workflow_path: workflowPath,
      workflow_ref: workflowRef,
      workflow_sha: workflowSha,
      run_id: runId,
      run_attempt: runAttempt,
      maximum_budget_micro_usd: maximumBudgetMicroUsd,
      maximum_runtime_seconds: maximumRuntimeSeconds
    },
    authority,
    files: {
      evidence: fileIdentity(FILES.evidence, evidence.bytes),
      receipt: fileIdentity(FILES.receipt, receipt.bytes)
    }
  };
  const output = join(root, BINDING_FILE);
  writeFileSync(output, canonicalJsonBytes(binding), { flag: "wx" });
  exactRegularFiles(root, [...Object.values(FILES), BINDING_FILE]);
  return { binding, digest: sha256(readFileSync(output)), output };
}

export function verifyBinding(options) {
  const directory = resolve(required(options, "directory"));
  const root = exactRegularFiles(directory, [...Object.values(FILES), BINDING_FILE]);
  const evidence = readCanonicalJson(join(root, FILES.evidence), FILES.evidence);
  const receipt = readCanonicalJson(join(root, FILES.receipt), FILES.receipt);
  const binding = readCanonicalJson(join(root, BINDING_FILE), BINDING_FILE);
  const authority = qualificationRelationships(evidence, receipt);

  const expectedDigest = expectPattern(required(options, "binding_sha256"), /^sha256:[0-9a-f]{64}$/, "binding_sha256");
  if (sha256(binding.bytes) !== expectedDigest) {
    fail("qualification binding bytes do not equal the approved binding_sha256");
  }
  const expectedRepository = expectPattern(required(options, "expected_repository"), /^aexhq\/aex$/, "expected_repository");
  const expectedSourceSha = expectPattern(required(options, "expected_source_sha"), /^[0-9a-f]{40}$/, "expected_source_sha");
  const expectedRunId = positiveInteger(required(options, "expected_run_id"), "expected_run_id");
  const expectedRunAttempt = positiveInteger(required(options, "expected_run_attempt"), "expected_run_attempt");
  const expectedWorkflowRef = `${expectedRepository}/${QUALIFIER_WORKFLOW}@${SOURCE_REF}`;
  const qualification = binding.value?.qualification;
  if (qualification === null || typeof qualification !== "object" || Array.isArray(qualification)) {
    fail("qualification binding is missing its qualification identity");
  }
  validateEvidence(evidence, receipt, {
    repository: expectedRepository,
    sourceSha: expectedSourceSha,
    runId: expectedRunId,
    runAttempt: expectedRunAttempt,
    maximumBudgetMicroUsd: qualification.maximum_budget_micro_usd,
    maximumRuntimeSeconds: qualification.maximum_runtime_seconds
  });
  const expected = {
    schema: SCHEMA,
    target: { provider: TARGET_PROVIDER, model: TARGET_MODEL },
    qualification: {
      repository: expectedRepository,
      source_sha: expectedSourceSha,
      source_ref: SOURCE_REF,
      workflow_path: QUALIFIER_WORKFLOW,
      workflow_ref: expectedWorkflowRef,
      workflow_sha: expectedSourceSha,
      run_id: expectedRunId,
      run_attempt: expectedRunAttempt,
      maximum_budget_micro_usd: positiveJsonInteger(
        qualification.maximum_budget_micro_usd,
        "qualification maximum_budget_micro_usd"
      ),
      maximum_runtime_seconds: positiveJsonInteger(
        qualification.maximum_runtime_seconds,
        "qualification maximum_runtime_seconds"
      )
    },
    authority,
    files: {
      evidence: fileIdentity(FILES.evidence, evidence.bytes),
      receipt: fileIdentity(FILES.receipt, receipt.bytes)
    }
  };
  if (!binding.bytes.equals(canonicalJsonBytes(expected))) {
    fail("qualification binding fields do not equal the acquired files and protected run identity");
  }
  return { binding: binding.value, digest: expectedDigest };
}

function main(argv) {
  const { command, options } = parseArguments(argv);
  const result = command === "create" ? createBinding(options) : verifyBinding(options);
  process.stdout.write(`${JSON.stringify({ ok: true, schema: SCHEMA, binding_sha256: result.digest })}\n`);
}

const invokedPath = process.argv[1] === undefined ? "" : resolve(process.argv[1]);
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`model-catalog qualification binding: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
