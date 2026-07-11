import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import semver from "semver";

const SOURCE_SHA_RE = /^[0-9a-f]{40}$/;
const CANARY_SOURCE_RE = /-canary\.\d+\.g([0-9a-f]{12})$/;
const REQUIRED_TAGS = ["latest", "canary"];
export const SOURCE_ATTESTATION_REQUIRED_FROM = "0.42.0";

function normalizeSha(value, label) {
  const normalized = String(value ?? "").toLowerCase();
  if (!SOURCE_SHA_RE.test(normalized)) {
    throw new Error(`invalid ${label} source SHA: ${value ?? "(missing)"}`);
  }
  return normalized;
}

function normalizeVersion(value, label) {
  const normalized = semver.valid(String(value ?? ""));
  if (normalized === null) throw new Error(`invalid ${label} version: ${value ?? "(missing)"}`);
  return normalized;
}

function releaseCore(version) {
  return `${semver.major(version)}.${semver.minor(version)}.${semver.patch(version)}`;
}

function assertCanarySource(version, sourceSha, label) {
  const sourcePrefix = CANARY_SOURCE_RE.exec(version)?.[1];
  if (sourcePrefix && !sourceSha.startsWith(sourcePrefix)) {
    throw new Error(`${label} version ${version} does not match source attestation ${sourceSha}`);
  }
}

export function normalizeTaggedRelease(tagged) {
  const tag = String(tagged?.tag ?? "");
  if (!REQUIRED_TAGS.includes(tag)) throw new Error(`unexpected registry tag: ${tag || "(missing)"}`);
  const version = normalizeVersion(tagged?.version, `current ${tag}`);
  const rawSourceSha = String(tagged?.sourceSha ?? "").trim();
  if (!rawSourceSha) {
    if (semver.lt(releaseCore(version), SOURCE_ATTESTATION_REQUIRED_FROM)) {
      return { tag, version, sourceSha: null, sourceProof: "legacy-unattested" };
    }
    throw new Error(`current ${tag} ${version} is missing source attestation`);
  }

  const sourceSha = normalizeSha(rawSourceSha, `current ${tag}`);
  assertCanarySource(version, sourceSha, `current ${tag}`);
  return { tag, version, sourceSha, sourceProof: "attested" };
}

function normalizeTaggedReleases(taggedReleases) {
  if (!Array.isArray(taggedReleases)) throw new Error("taggedReleases must be an array");
  const normalized = taggedReleases.map(normalizeTaggedRelease);
  for (const requiredTag of REQUIRED_TAGS) {
    if (normalized.filter(({ tag }) => tag === requiredTag).length !== 1) {
      throw new Error(`registry source proof requires exactly one ${requiredTag} tag`);
    }
  }
  if (normalized.length !== REQUIRED_TAGS.length) {
    throw new Error(`registry source proof requires only ${REQUIRED_TAGS.join(" and ")} tags`);
  }
  return normalized;
}

export function assertMonotonicPromotion({
  candidateVersion,
  candidateSha,
  currentMainSha,
  candidateIsMainAncestor,
  taggedReleases
}) {
  const normalizedCandidateSha = normalizeSha(candidateSha, "candidate");
  const normalizedMainSha = normalizeSha(currentMainSha, "current main");
  if (candidateIsMainAncestor !== true) {
    throw new Error(
      `candidate source ${normalizedCandidateSha} is not an ancestor of current main ${normalizedMainSha}`
    );
  }

  const normalizedCandidateVersion = normalizeVersion(candidateVersion, "candidate");
  assertCanarySource(normalizedCandidateVersion, normalizedCandidateSha, "candidate");
  const normalizedTags = normalizeTaggedReleases(taggedReleases);

  for (const tagged of normalizedTags) {
    if (semver.lt(normalizedCandidateVersion, tagged.version)) {
      throw new Error(
        `candidate version ${normalizedCandidateVersion} is older than current ${tagged.tag} ${tagged.version}`
      );
    }
    if (tagged.sourceProof === "attested") {
      const input = taggedReleases.find(({ tag }) => tag === tagged.tag);
      if (input?.sourceIsCandidateAncestor !== true) {
        throw new Error(
          `current ${tagged.tag} source ${tagged.sourceSha} is not behind the candidate ${normalizedCandidateSha}`
        );
      }
    }
  }

  return {
    candidateVersion: normalizedCandidateVersion,
    candidateSha: normalizedCandidateSha,
    currentMainSha: normalizedMainSha,
    taggedReleases: normalizedTags
  };
}

function isAncestor(repository, ancestorSha, descendantSha, label) {
  const result = spawnSync("git", ["merge-base", "--is-ancestor", ancestorSha, descendantSha], {
    cwd: repository,
    encoding: "utf8"
  });
  if (result.status === 0) return true;
  if (result.status === 1) return false;
  const detail = String(result.stderr || result.stdout || "unknown git error").trim();
  throw new Error(`could not verify ${label} ancestry: ${detail}`);
}

export function assertPromotionInRepository({
  repository = process.cwd(),
  candidateVersion,
  candidateSha,
  currentMainSha,
  taggedReleases
}) {
  const normalizedCandidateSha = normalizeSha(candidateSha, "candidate");
  const normalizedMainSha = normalizeSha(currentMainSha, "current main");
  const normalizedTags = normalizeTaggedReleases(taggedReleases);
  const relations = new Map(
    normalizedTags
      .filter(({ sourceProof }) => sourceProof === "attested")
      .map(({ tag, sourceSha }) => [
        tag,
        isAncestor(repository, sourceSha, normalizedCandidateSha, `current ${tag} source`)
      ])
  );

  return assertMonotonicPromotion({
    candidateVersion,
    candidateSha: normalizedCandidateSha,
    currentMainSha: normalizedMainSha,
    candidateIsMainAncestor: isAncestor(
      repository,
      normalizedCandidateSha,
      normalizedMainSha,
      "candidate/current main"
    ),
    taggedReleases: taggedReleases.map((tagged) => ({
      ...tagged,
      sourceIsCandidateAncestor: relations.get(tagged.tag)
    }))
  });
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
  const result = assertPromotionInRepository({
    repository: resolve(args.repository || process.cwd()),
    candidateVersion: required(args, "candidateVersion"),
    candidateSha: required(args, "candidateSha"),
    currentMainSha: required(args, "currentMainSha"),
    taggedReleases: [
      {
        tag: "latest",
        version: required(args, "currentLatestVersion"),
        sourceSha: args.currentLatestSha ?? ""
      },
      {
        tag: "canary",
        version: required(args, "currentCanaryVersion"),
        sourceSha: args.currentCanarySha ?? ""
      }
    ]
  });
  const sources = result.taggedReleases
    .map(({ tag, sourceSha, sourceProof }) => `${tag}=${sourceSha ?? sourceProof}`)
    .join(", ");
  process.stdout.write(
    `Promotion candidate ${result.candidateVersion} at ${result.candidateSha} is source-monotonic with main and registry tags (${sources}).\n`
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
