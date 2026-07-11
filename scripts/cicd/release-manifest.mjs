import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PUBLIC_RELEASE_MANIFEST_KIND = "aex-public-release-manifest";
export const PLATFORM_VALIDATION_MANIFEST_KIND = "aex-platform-validation-manifest";
export const MANIFEST_SCHEMA_VERSION = 3;
export const REQUIRED_PLATFORM_GATES = ["suite_dev", "spot_canary_dev", "suite_prod", "smoke_prod"];

export function buildPublicReleaseManifest(input) {
  const now = input.createdAt ?? new Date().toISOString();
  return {
    schemaVersion: MANIFEST_SCHEMA_VERSION,
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
  if (manifest?.schemaVersion !== MANIFEST_SCHEMA_VERSION) errors.push("schemaVersion must be 3");
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
  if (!manifest?.sdk?.initialDistTag) errors.push("sdk.initialDistTag is required");
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
  if (manifest?.schemaVersion !== MANIFEST_SCHEMA_VERSION) errors.push("schemaVersion must be 3");
  if (manifest?.kind !== PLATFORM_VALIDATION_MANIFEST_KIND) {
    errors.push(`kind must be ${PLATFORM_VALIDATION_MANIFEST_KIND}`);
  }
  if (expected.version && manifest?.sdk?.version !== expected.version) {
    errors.push(`sdk.version must be ${expected.version}`);
  }
  if (!manifest?.sdk?.integrity) errors.push("sdk.integrity is required");
  if (expected.runId && String(manifest?.platform?.runId ?? "") !== String(expected.runId)) {
    errors.push(`platform.runId must be ${expected.runId}`);
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
  for (const gate of REQUIRED_PLATFORM_GATES) {
    if (!Array.isArray(manifest?.gates) || !manifest.gates.includes(gate)) {
      errors.push(`missing platform gate ${gate}`);
    }
  }
  for (const image of ["brain", "egress", "byok"]) {
    if (!manifest?.images?.[image]?.tag) errors.push(`images.${image}.tag is required`);
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
