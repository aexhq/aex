#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const GATES = ["lint", "unit_tests", "offline_user_tests", "docs_build", "pack_sdk"];

const ZERO_SHA_RE = /^0{40}$/;
const GIT_LOCAL_ENV_VARS = [
  "GIT_ALTERNATE_OBJECT_DIRECTORIES",
  "GIT_COMMON_DIR",
  "GIT_CONFIG",
  "GIT_CONFIG_COUNT",
  "GIT_CONFIG_PARAMETERS",
  "GIT_DIR",
  "GIT_GRAFT_FILE",
  "GIT_IMPLICIT_WORK_TREE",
  "GIT_INDEX_FILE",
  "GIT_NO_REPLACE_OBJECTS",
  "GIT_OBJECT_DIRECTORY",
  "GIT_PREFIX",
  "GIT_REPLACE_REF_BASE",
  "GIT_SHALLOW_FILE",
  "GIT_WORK_TREE"
];

export function analyzeFeatureGate(paths) {
  const changedPaths = [...new Set(paths.map(normalizePath).filter(Boolean))];
  const gates = Object.fromEntries(GATES.map((gate) => [gate, false]));
  const reasons = Object.fromEntries(GATES.map((gate) => [gate, []]));

  function add(gate, path, reason) {
    gates[gate] = true;
    reasons[gate].push({ path, reason });
  }

  function addMany(gateNames, path, reason) {
    for (const gate of gateNames) add(gate, path, reason);
  }

  for (const path of changedPaths) {
    if (isGlobalBuildConfig(path)) {
      addMany(GATES, path, "global build/test configuration changed");
      continue;
    }
    if (isWorkflowOrCicd(path)) {
      addMany(["lint", "unit_tests", "offline_user_tests", "pack_sdk"], path, "CI/release automation changed");
      continue;
    }
    if (isPublicContract(path)) {
      addMany(["lint", "unit_tests", "offline_user_tests", "docs_build", "pack_sdk"], path, "public contract changed");
      continue;
    }
    if (isDocsSource(path)) {
      addMany(["lint", "docs_build"], path, "documentation source changed");
      continue;
    }
    if (path.startsWith("packages/sdk/")) {
      addMany(["lint", "unit_tests", "offline_user_tests", "docs_build", "pack_sdk"], path, "SDK package changed");
      continue;
    }
    if (path.startsWith("packages/cli/")) {
      addMany(["lint", "unit_tests", "offline_user_tests", "pack_sdk"], path, "CLI package changed");
      continue;
    }
    if (path.startsWith("packages/conformance/")) {
      addMany(["lint", "unit_tests", "offline_user_tests", "pack_sdk"], path, "conformance package changed");
      continue;
    }
    if (path.startsWith("apps/user-tests/")) {
      addMany(["lint", "unit_tests", "offline_user_tests"], path, "user-test harness changed");
      continue;
    }
    if (isSourceLike(path)) {
      addMany(["lint", "unit_tests"], path, "source-like file changed");
    }
  }

  if (changedPaths.length === 0) {
    add("lint", "(none)", "no changed paths supplied; run the cheapest structural gate");
  }

  return {
    changedPaths,
    gates,
    reasons: Object.fromEntries(GATES.map((gate) => [gate, uniqueReasons(reasons[gate])])),
    commands: {
      lint: "bun run lint",
      unit_tests: "bun run test",
      offline_user_tests: "bun run test:user:offline",
      docs_build: "bun run docs:build",
      pack_sdk: "bun run pack:sdk"
    }
  };
}

export function normalizePath(path) {
  return String(path ?? "").replaceAll("\\", "/").replace(/^\.\//, "").trim();
}

function isGlobalBuildConfig(path) {
  return [
    "package.json",
    "bun.lock",
    "tsconfig.base.json",
    "tsconfig.scripts.json",
    "eslint.config.mjs",
    "vitest.workspace.ts"
  ].includes(path);
}

function isWorkflowOrCicd(path) {
  return path.startsWith(".github/workflows/") || path.startsWith("scripts/cicd/") || path.startsWith("scripts/validate/");
}

function isPublicContract(path) {
  return path === "packages/contracts/package.json" || path.startsWith("packages/contracts/src/");
}

function isDocsSource(path) {
  return (
    path.startsWith("apps/docs/") ||
    path.startsWith("packages/sdk/docs/") ||
    ["README.md", "CONTRIBUTING.md", "SECURITY.md"].includes(path) ||
    path.startsWith("scripts/docs/")
  );
}

function isSourceLike(path) {
  return /\.(?:[cm]?[jt]sx?|json|md|ya?ml)$/.test(path);
}

function uniqueReasons(entries) {
  const seen = new Set();
  return entries.filter((entry) => {
    const key = `${entry.path}\0${entry.reason}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function parseArgs(argv) {
  const args = { paths: [], base: null, head: "HEAD", githubOutput: process.env.GITHUB_OUTPUT ?? "" };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--base") {
      args.base = argv[++i] ?? null;
    } else if (arg === "--head") {
      args.head = argv[++i] ?? "HEAD";
    } else if (arg === "--paths") {
      args.paths.push(...(argv[++i] ?? "").split(",").filter(Boolean));
    } else if (arg === "--github-output") {
      args.githubOutput = argv[++i] ?? process.env.GITHUB_OUTPUT ?? "";
    } else {
      args.paths.push(arg);
    }
  }
  return args;
}

function changedPathsFromGit({ base, head }) {
  if (ZERO_SHA_RE.test(base)) return trackedPathsFromGit(head);
  const out = execFileSync("git", ["diff", "--name-only", `${base}...${head}`], {
    env: withoutGitLocalEnv(process.env),
    encoding: "utf8"
  });
  return out.split(/\r?\n/).filter(Boolean);
}

function trackedPathsFromGit(head) {
  const out = execFileSync("git", ["ls-tree", "-r", "--name-only", head], {
    env: withoutGitLocalEnv(process.env),
    encoding: "utf8"
  });
  return out.split(/\r?\n/).filter(Boolean);
}

function writeGithubOutputs(analysis, outputPath) {
  if (!outputPath) return;
  const lines = GATES.map((gate) => `${gate}=${analysis.gates[gate] ? "true" : "false"}`);
  lines.push(`json=${JSON.stringify(analysis)}`);
  appendFileSync(outputPath, `${lines.join("\n")}\n`, "utf8");
}

function withoutGitLocalEnv(env) {
  const scrubbed = { ...env };
  for (const key of GIT_LOCAL_ENV_VARS) deleteEnvCaseInsensitive(scrubbed, key);
  return scrubbed;
}

function deleteEnvCaseInsensitive(env, key) {
  const wanted = key.toUpperCase();
  for (const existing of Object.keys(env)) {
    if (existing.toUpperCase() === wanted) delete env[existing];
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = parseArgs(process.argv.slice(2));
  const paths = args.paths.length > 0
    ? args.paths
    : args.base
      ? changedPathsFromGit({ base: args.base, head: args.head })
      : [];
  const analysis = analyzeFeatureGate(paths);
  writeGithubOutputs(analysis, args.githubOutput);
  console.log(JSON.stringify(analysis, null, 2));
}
