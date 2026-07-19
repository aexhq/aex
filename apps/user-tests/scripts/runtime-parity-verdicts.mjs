import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export const PARITY_SCENARIO_OWNERSHIP = Object.freeze({
  "test/live/edge-cli.user.test.ts": Object.freeze({
    layer: "user",
    entryPoint: "cli",
    scenarioIds: Object.freeze([
      "public.admission-and-identity",
      "public.conversation",
      "public.files-and-checkpoints"
    ])
  }),
  "test/live/live-sdk-event-stream.test.ts": Object.freeze({
    layer: "user",
    entryPoint: "sdk",
    scenarioIds: Object.freeze([
      "public.admission-and-identity",
      "public.conversation",
      "public.files-and-checkpoints"
    ])
  })
});

export function parityCellsForFile(file, runtimeKind) {
  const owner = PARITY_SCENARIO_OWNERSHIP[file];
  if (!owner || runtimeKind === null) return [];
  return owner.scenarioIds.map((scenarioId) => ({
    scenarioId,
    layer: owner.layer,
    entryPoint: owner.entryPoint,
    runtime: runtimeKind
  }));
}

export function emitVerdicts({ cells, stepOutcome, candidateIdentity, reportPath, outputPath }) {
  if (!Array.isArray(cells) || cells.length === 0) throw new Error("verdict emission requires parity cells");
  if (!isCandidateIdentity(candidateIdentity)) throw new Error("candidate identity is missing or malformed");
  if (!existsSync(reportPath)) throw new Error(`test report is missing: ${reportPath}`);
  const status = stepOutcome === "success" ? "passed" : "failed";
  const cleanup = status === "passed" ? "passed" : "pending";
  const evidenceDigest = `sha256:${createHash("sha256").update(readFileSync(reportPath)).digest("hex")}`;
  const verdicts = cells.map((cell) => ({
    ...cell,
    status,
    cleanup,
    evidenceDigest,
    candidateIdentity
  }));
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(verdicts, null, 2)}\n`);
  return verdicts;
}

export function validateVerdictDirectory({ matrix, verdictDirectory, candidateIdentity }) {
  if (!Array.isArray(matrix) || matrix.length === 0) throw new Error("runtime matrix is empty");
  if (!isCandidateIdentity(candidateIdentity)) throw new Error("candidate identity is missing or malformed");
  const expected = matrix.flatMap((entry) => Array.isArray(entry.parityCells) ? entry.parityCells : []);
  if (expected.length === 0) throw new Error("runtime matrix contains no parity cells");
  const files = readdirSync(verdictDirectory).filter((name) => name.endsWith(".json")).sort();
  const actual = files.flatMap((name) => {
    const parsed = JSON.parse(readFileSync(join(verdictDirectory, name), "utf8"));
    if (!Array.isArray(parsed)) throw new Error(`${name} must contain a verdict array`);
    return parsed;
  });
  const failures = [];
  const expectedKeys = new Set(expected.map(cellKey));
  const actualByKey = new Map();
  for (const verdict of actual) {
    const key = cellKey(verdict);
    const matches = actualByKey.get(key) ?? [];
    matches.push(verdict);
    actualByKey.set(key, matches);
    if (!expectedKeys.has(key)) failures.push(`unexpected verdict: ${key}`);
    if (verdict.status !== "passed") failures.push(`non-passing verdict: ${key} status=${String(verdict.status)}`);
    if (verdict.cleanup !== "passed") failures.push(`cleanup verdict: ${key} cleanup=${String(verdict.cleanup)}`);
    if (!/^sha256:[0-9a-f]{64}$/.test(String(verdict.evidenceDigest))) failures.push(`invalid evidence digest: ${key}`);
    if (verdict.candidateIdentity !== candidateIdentity) failures.push(`candidate identity mismatch: ${key}`);
  }
  for (const cell of expected) {
    const key = cellKey(cell);
    const verdicts = actualByKey.get(key) ?? [];
    if (verdicts.length === 0) failures.push(`missing verdict: ${key}`);
    if (verdicts.length > 1) failures.push(`duplicate verdict: ${key}`);
  }
  if (new Set(expected.map(cellKey)).size !== expected.length) failures.push("runtime matrix contains duplicate expected parity cells");
  if (failures.length > 0) throw new Error(`runtime parity verdict closure failed:\n${failures.sort().join("\n")}`);
  return { expected: expected.length, actual: actual.length };
}

function cellKey(cell) {
  return `${cell?.scenarioId}/${cell?.layer}/${cell?.entryPoint}/${cell?.runtime}`;
}

function isCandidateIdentity(value) {
  return typeof value === "string" && (/^sha256:[0-9a-f]{64}$/.test(value) || /^npm:@aexhq\/sdk@[^\s]+$/.test(value));
}

function argumentMap(argv) {
  const result = new Map();
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i];
    const value = argv[i + 1];
    if (!name?.startsWith("--") || value === undefined) throw new Error(`invalid argument near ${String(name)}`);
    result.set(name, value);
  }
  return result;
}

function required(args, name) {
  const value = args.get(name);
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function main(argv) {
  const [command, ...rest] = argv;
  const args = argumentMap(rest);
  if (command === "emit") {
    emitVerdicts({
      cells: JSON.parse(required(args, "--cells-json")),
      stepOutcome: required(args, "--step-outcome"),
      candidateIdentity: required(args, "--candidate-identity"),
      reportPath: resolve(required(args, "--report")),
      outputPath: resolve(required(args, "--out"))
    });
    return;
  }
  if (command === "validate") {
    const result = validateVerdictDirectory({
      matrix: JSON.parse(required(args, "--matrix-json")),
      verdictDirectory: resolve(required(args, "--verdict-directory")),
      candidateIdentity: required(args, "--candidate-identity")
    });
    process.stdout.write(`runtime parity verdict closure passed (${result.actual}/${result.expected}).\n`);
    return;
  }
  throw new Error("usage: runtime-parity-verdicts.mjs emit|validate [options]");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`runtime-parity-verdicts: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
