import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PUBLIC_RELEASE_MANIFEST_KIND = "aex-public-release-manifest";
export const PLATFORM_VALIDATION_MANIFEST_KIND = "platform-validation-manifest";
export const PUBLIC_MANIFEST_SCHEMA_VERSION = 3;
export const PLATFORM_MANIFEST_SCHEMA_VERSION = 5;
export const MANIFEST_SCHEMA_VERSION = PUBLIC_MANIFEST_SCHEMA_VERSION;
export const REQUIRED_PLATFORM_GATES = ["suite_dev", "spot_canary_dev", "suite_prod", "smoke_prod"];
const IMAGE_DIGEST_RE = /^sha256:[0-9a-f]{64}$/;
const BRAIN_TAG_RE = /^sha-([0-9a-f]{12})-pub-([0-9a-f]{12})$/;
const SIDECAR_TAG_RE = /^sha-[0-9a-f]{12}$/;
const AWS_REGION = "eu-west-2";

export function buildPublicReleaseManifest(input) {
  const now = input.createdAt ?? new Date().toISOString();
  return {
    schemaVersion: PUBLIC_MANIFEST_SCHEMA_VERSION,
    kind: PUBLIC_RELEASE_MANIFEST_KIND,
    createdAt: now,
    repository: input.repository ?? process.env.GITHUB_REPOSITORY ?? "aexhq/aex",
    workflow: "release.yml",
    runId: String(input.runId ?? process.env.GITHUB_RUN_ID ?? ""),
    runAttempt: String(input.runAttempt ?? process.env.GITHUB_RUN_ATTEMPT ?? ""),
    headSha: input.headSha ?? process.env.GITHUB_SHA ?? "",
    sdk: {
      packageName: "@aexhq/sdk",
      version: input.version,
      integrity: input.integrity,
      initialDistTag: input.distTag
    },
    cli: {
      packageName: "@aexhq/sdk",
      version: input.version,
      integrity: input.integrity,
      bin: "aex"
    },
    gates: [
      "lint",
      "unit-tests",
      "offline-user-tests",
      "docs-build",
      "pack-sdk",
      "live-user-tests-preflight",
      "publish",
      "published-artifact-smoke"
    ],
    evidence: {
      releaseSmokeArtifact: input.releaseSmokeArtifact ?? "release-smoke-redacted-log"
    },
    promotion: {
      status: "candidate"
    }
  };
}

export function validatePublicReleaseManifest(manifest, expected = {}) {
  const errors = [];
  if (!isRecord(manifest)) errors.push("manifest must be an object");
  if (manifest?.schemaVersion !== PUBLIC_MANIFEST_SCHEMA_VERSION) errors.push("schemaVersion must be 3");
  if (manifest?.kind !== PUBLIC_RELEASE_MANIFEST_KIND) errors.push(`kind must be ${PUBLIC_RELEASE_MANIFEST_KIND}`);
  if (expected.version && manifest?.sdk?.version !== expected.version) {
    errors.push(`sdk.version must be ${expected.version}`);
  }
  if (expected.runId && String(manifest?.runId ?? "") !== String(expected.runId)) {
    errors.push(`runId must be ${expected.runId}`);
  }
  if (expected.headSha && manifest?.headSha !== expected.headSha) {
    errors.push(`headSha must be ${expected.headSha}`);
  }
  if (expected.integrity && manifest?.sdk?.integrity !== expected.integrity) {
    errors.push(`sdk.integrity must be ${expected.integrity}`);
  }
  if (manifest?.repository !== "aexhq/aex") errors.push("repository must be aexhq/aex");
  if (manifest?.workflow !== "release.yml") errors.push("workflow must be release.yml");
  if (!manifest?.headSha) errors.push("headSha is required");
  if (manifest?.sdk?.packageName !== "@aexhq/sdk") errors.push("sdk.packageName must be @aexhq/sdk");
  if (!manifest?.sdk?.integrity) errors.push("sdk.integrity is required");
  if (manifest?.sdk?.initialDistTag !== "canary") errors.push("sdk.initialDistTag must be canary");
  if (manifest?.cli?.packageName !== "@aexhq/sdk") errors.push("cli.packageName must be @aexhq/sdk");
  if (manifest?.cli?.version !== manifest?.sdk?.version) errors.push("cli.version must match sdk.version");
  if (manifest?.cli?.integrity !== manifest?.sdk?.integrity) errors.push("cli.integrity must match sdk.integrity");
  if (manifest?.cli?.bin !== "aex") errors.push("cli.bin must be aex");
  for (const gate of ["publish", "published-artifact-smoke"]) {
    if (!Array.isArray(manifest?.gates) || !manifest.gates.includes(gate)) {
      errors.push(`missing release gate ${gate}`);
    }
  }
  return { ok: errors.length === 0, errors };
}

export function validatePlatformValidationManifest(manifest, expected = {}) {
  const errors = [];
  if (!isRecord(manifest)) errors.push("manifest must be an object");
  if (manifest?.schemaVersion !== PLATFORM_MANIFEST_SCHEMA_VERSION) errors.push("schemaVersion must be 5");
  if (manifest?.kind !== PLATFORM_VALIDATION_MANIFEST_KIND) {
    errors.push(`kind must be ${PLATFORM_VALIDATION_MANIFEST_KIND}`);
  }
  if (expected.version && manifest?.sdk?.version !== expected.version) {
    errors.push(`sdk.version must be ${expected.version}`);
  }
  if (!manifest?.sdk?.integrity) errors.push("sdk.integrity is required");
  if (expected.runId && String(manifest?.productionPromotion?.runId ?? "") !== String(expected.runId)) {
    errors.push(`productionPromotion.runId must be ${expected.runId}`);
  }
  if (expected.devValidationRunId && String(manifest?.devValidation?.runId ?? "") !== String(expected.devValidationRunId)) {
    errors.push(`devValidation.runId must be ${expected.devValidationRunId}`);
  }
  if (expected.publicReleaseRunId && String(manifest?.publicRelease?.runId ?? "") !== String(expected.publicReleaseRunId)) {
    errors.push(`publicRelease.runId must be ${expected.publicReleaseRunId}`);
  }
  if (expected.publicReleaseHeadSha && manifest?.publicRelease?.headSha !== expected.publicReleaseHeadSha) {
    errors.push(`publicRelease.headSha must be ${expected.publicReleaseHeadSha}`);
  }
  if (expected.integrity && manifest?.sdk?.integrity !== expected.integrity) {
    errors.push(`sdk.integrity must be ${expected.integrity}`);
  }
  if (!/^[1-9]\d*$/.test(String(manifest?.devValidation?.runId ?? ""))) {
    errors.push("devValidation.runId must be a positive GitHub run id");
  }
  if (!/^[1-9]\d*$/.test(String(manifest?.productionPromotion?.runId ?? ""))) {
    errors.push("productionPromotion.runId must be a positive GitHub run id");
  }
  if (!/^[1-9]\d*$/.test(String(manifest?.productionPromotion?.runAttempt ?? ""))) {
    errors.push("productionPromotion.runAttempt must be positive");
  }
  if (manifest?.devValidation?.repository !== "aexhq/platform") {
    errors.push("devValidation.repository must be aexhq/platform");
  }
  if (manifest?.devValidation?.workflow !== "deploy-dev.yml") {
    errors.push("devValidation.workflow must be deploy-dev.yml");
  }
  if (manifest?.productionPromotion?.repository !== "aexhq/platform") {
    errors.push("productionPromotion.repository must be aexhq/platform");
  }
  if (manifest?.productionPromotion?.workflow !== "promote-prd.yml") {
    errors.push("productionPromotion.workflow must be promote-prd.yml");
  }
  const devPlatformSha = String(manifest?.devValidation?.headSha ?? "");
  const platformSha = String(manifest?.productionPromotion?.headSha ?? "");
  if (!/^[0-9a-f]{40}$/.test(devPlatformSha)) {
    errors.push("devValidation.headSha must be a full lowercase git SHA");
  }
  if (!/^[0-9a-f]{40}$/.test(platformSha)) {
    errors.push("productionPromotion.headSha must be a full lowercase git SHA");
  }
  if (devPlatformSha && platformSha && devPlatformSha !== platformSha) {
    errors.push("productionPromotion.headSha must match the dev-tested platform SHA");
  }
  for (const gate of REQUIRED_PLATFORM_GATES) {
    if (!Array.isArray(manifest?.gates) || !manifest.gates.includes(gate)) {
      errors.push(`missing platform gate ${gate}`);
    }
  }
  const publicSha = String(manifest?.publicRelease?.headSha ?? "");
  for (const [image, repositoryKind] of [["brain", "brain"], ["egress", "egress-proxy"], ["byok", "byok-inject"]]) {
    const evidence = manifest?.images?.[image];
    const tag = String(evidence?.tag ?? "");
    if (!tag) {
      errors.push(`images.${image}.tag is required`);
    } else if (image === "brain") {
      const match = BRAIN_TAG_RE.exec(tag);
      if (!match) errors.push("images.brain.tag must match sha-<platform12>-pub-<public12>");
      else {
        if (/^[0-9a-f]{40}$/.test(platformSha) && match[1] !== platformSha.slice(0, 12)) {
          errors.push("images.brain.tag platform source must match productionPromotion.headSha");
        }
        if (/^[0-9a-f]{40}$/.test(publicSha) && match[2] !== publicSha.slice(0, 12)) {
          errors.push("images.brain.tag public source must match publicRelease.headSha");
        }
      }
    } else if (!SIDECAR_TAG_RE.test(tag)) {
      errors.push(`images.${image}.tag must match sha-<12hex>`);
    }
    for (const plane of ["dev", "prd"]) {
      const expectedRepository = `aex-${plane}-${AWS_REGION}-${repositoryKind}`;
      if (evidence?.[plane]?.repository !== expectedRepository) {
        errors.push(`images.${image}.${plane}.repository must be ${expectedRepository}`);
      }
      if (!IMAGE_DIGEST_RE.test(String(evidence?.[plane]?.digest ?? ""))) {
        errors.push(`images.${image}.${plane}.digest must be a sha256 image digest`);
      }
    }
    if (evidence?.dev?.digest !== evidence?.prd?.digest) {
      errors.push(`images.${image} prd digest must match the dev-tested digest`);
    }
  }
  const lambdaEvidence = manifest?.evidence?.lambdaZips;
  if (!/^[1-9]\d*$/.test(String(lambdaEvidence?.id ?? ""))) {
    errors.push("evidence.lambdaZips.id must be a positive artifact id");
  }
  if (!IMAGE_DIGEST_RE.test(String(lambdaEvidence?.digest ?? ""))) {
    errors.push("evidence.lambdaZips.digest must be a sha256 artifact digest");
  }
  if (String(lambdaEvidence?.sourceRunId ?? "") !== String(manifest?.devValidation?.runId ?? "")) {
    errors.push("evidence.lambdaZips.sourceRunId must match devValidation.runId");
  }
  if (manifest?.planes?.dev?.apiBase !== "https://dev-api.aex.dev") {
    errors.push("planes.dev.apiBase must be https://dev-api.aex.dev");
  }
  if (manifest?.planes?.prd?.apiBase !== "https://api.aex.dev") {
    errors.push("planes.prd.apiBase must be https://api.aex.dev");
  }
  return { ok: errors.length === 0, errors };
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command };
  for (let i = 0; i < rest.length; i += 1) {
    const key = rest[i];
    if (!key.startsWith("--")) throw new Error(`unexpected argument: ${key}`);
    args[key.slice(2).replace(/-([a-z])/g, (_m, c) => c.toUpperCase())] = rest[++i] ?? "";
  }
  return args;
}

function requireArg(args, name) {
  if (!args[name]) throw new Error(`--${name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)} is required`);
  return args[name];
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeJson(path, value) {
  mkdirSync(dirname(resolve(path)), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.command === "write-public") {
    const manifest = buildPublicReleaseManifest({
      version: requireArg(args, "version"),
      distTag: requireArg(args, "distTag"),
      integrity: requireArg(args, "integrity"),
      headSha: args.headSha
    });
    writeJson(requireArg(args, "out"), manifest);
    return manifest;
  }
  if (args.command === "verify-public") {
    const result = validatePublicReleaseManifest(readJson(requireArg(args, "manifest")), {
      version: requireArg(args, "version"),
      runId: requireArg(args, "runId"),
      headSha: requireArg(args, "headSha"),
      integrity: requireArg(args, "integrity")
    });
    if (!result.ok) throw new Error(`public release manifest invalid: ${result.errors.join("; ")}`);
    console.log("public release manifest: ok");
    return result;
  }
  if (args.command === "verify-platform") {
    const result = validatePlatformValidationManifest(readJson(requireArg(args, "manifest")), {
      version: requireArg(args, "version"),
      runId: requireArg(args, "runId"),
      devValidationRunId: requireArg(args, "devValidationRunId"),
      publicReleaseRunId: requireArg(args, "publicReleaseRunId"),
      publicReleaseHeadSha: requireArg(args, "publicReleaseHeadSha"),
      integrity: requireArg(args, "integrity")
    });
    if (!result.ok) throw new Error(`platform validation manifest invalid: ${result.errors.join("; ")}`);
    console.log("platform validation manifest: ok");
    return result;
  }
  throw new Error(`unknown command: ${args.command ?? "(missing)"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}
