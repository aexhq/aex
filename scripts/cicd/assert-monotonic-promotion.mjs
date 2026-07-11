import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import semver from "semver";

const SOURCE_SHA_RE = /^[0-9a-f]{40,64}$/;

export function assertMonotonicPromotion({
  candidateVersion,
  candidateSha,
  currentMainSha,
  candidateIsMainAncestor,
  currentLatestVersion
}) {
  const normalizedCandidateSha = String(candidateSha ?? "").toLowerCase();
  const normalizedMainSha = String(currentMainSha ?? "").toLowerCase();
  if (!SOURCE_SHA_RE.test(normalizedCandidateSha)) {
    throw new Error(`invalid candidate source SHA: ${candidateSha ?? "(missing)"}`);
  }
  if (!SOURCE_SHA_RE.test(normalizedMainSha)) {
    throw new Error(`invalid current main source SHA: ${currentMainSha ?? "(missing)"}`);
  }
  if (candidateIsMainAncestor !== true) {
    throw new Error(
      `candidate source ${normalizedCandidateSha} is not an ancestor of current main ${normalizedMainSha}`
    );
  }

  const normalizedCandidateVersion = semver.valid(String(candidateVersion ?? ""));
  if (normalizedCandidateVersion === null) {
    throw new Error(`invalid candidate version: ${candidateVersion ?? "(missing)"}`);
  }
  const normalizedLatestVersion = semver.valid(String(currentLatestVersion ?? ""));
  if (normalizedLatestVersion === null) {
    throw new Error(`invalid current latest version: ${currentLatestVersion ?? "(missing)"}`);
  }
  if (semver.lt(normalizedCandidateVersion, normalizedLatestVersion)) {
    throw new Error(
      `candidate version ${normalizedCandidateVersion} is older than current latest ${normalizedLatestVersion}`
    );
  }

  return {
    candidateVersion: normalizedCandidateVersion,
    candidateSha: normalizedCandidateSha,
    currentMainSha: normalizedMainSha,
    currentLatestVersion: normalizedLatestVersion
  };
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!key?.startsWith("--")) throw new Error(`unexpected argument: ${key ?? "(missing)"}`);
    const value = argv[++index];
    if (value === undefined) throw new Error(`${key} requires a value`);
    args[key.slice(2).replace(/-([a-z])/g, (_match, char) => char.toUpperCase())] = value;
  }
  return args;
}

function required(args, key) {
  const value = args[key];
  if (!value) throw new Error(`--${key.replace(/[A-Z]/g, (char) => `-${char.toLowerCase()}`)} is required`);
  return value;
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  const result = assertMonotonicPromotion({
    candidateVersion: required(args, "candidateVersion"),
    candidateSha: required(args, "candidateSha"),
    currentMainSha: required(args, "currentMainSha"),
    candidateIsMainAncestor: required(args, "candidateIsMainAncestor") === "true",
    currentLatestVersion: required(args, "currentLatestVersion")
  });
  process.stdout.write(
    `Promotion candidate ${result.candidateVersion} at ${result.candidateSha} is an ancestor of main ${result.currentMainSha} and not older than latest ${result.currentLatestVersion}.\n`
  );
  return result;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
