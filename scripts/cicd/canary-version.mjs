import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER_CORE_RE = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const CANARY_VERSION_RE = /^\d+\.\d+\.\d+-canary$/;

export function buildCanaryVersion({ baseVersion, sha }) {
  const match = SEMVER_CORE_RE.exec(String(baseVersion ?? ""));
  if (!match) throw new Error(`invalid base version: ${baseVersion ?? "(missing)"}`);

  const normalizedSha = String(sha ?? "").toLowerCase();
  if (!/^[0-9a-f]{40}$/.test(normalizedSha)) {
    throw new Error(`invalid source SHA: ${sha ?? "(missing)"}`);
  }

  return `${match[1]}.${match[2]}.${match[3]}-canary`;
}

export function applySdkVersion(repoRoot, version) {
  if (!CANARY_VERSION_RE.test(version)) {
    throw new Error(`refusing to apply invalid canary version: ${version}`);
  }

  const packagePath = resolve(repoRoot, "packages", "sdk", "package.json");
  const versionSourcePath = resolve(repoRoot, "packages", "sdk", "src", "version.ts");
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
  if (packageJson.name !== "@aexhq/sdk") {
    throw new Error(`unexpected package at ${packagePath}: ${packageJson.name ?? "(missing name)"}`);
  }
  packageJson.version = version;
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, "utf8");

  const versionSource = readFileSync(versionSourcePath, "utf8");
  const replacement = versionSource.replace(
    /export const SDK_VERSION = "[^"]+";/,
    `export const SDK_VERSION = "${version}";`
  );
  if (replacement === versionSource) {
    throw new Error(`SDK_VERSION export was not found in ${versionSourcePath}`);
  }
  writeFileSync(versionSourcePath, replacement, "utf8");
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command };
  for (let index = 0; index < rest.length; index += 1) {
    const key = rest[index];
    if (!key.startsWith("--")) throw new Error(`unexpected argument: ${key}`);
    args[key.slice(2).replace(/-([a-z])/g, (_match, char) => char.toUpperCase())] = rest[++index] ?? "";
  }
  return args;
}

function required(args, key) {
  if (!args[key]) throw new Error(`--${key.replace(/[A-Z]/g, (char) => `-${char.toLowerCase()}`)} is required`);
  return args[key];
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.command === "resolve") {
    const version = buildCanaryVersion({
      baseVersion: required(args, "baseVersion"),
      sha: required(args, "sha")
    });
    process.stdout.write(`${version}\n`);
    return version;
  }
  if (args.command === "apply") {
    const version = required(args, "version");
    applySdkVersion(resolve(args.repoRoot || process.cwd()), version);
    process.stdout.write(`Applied immutable SDK canary version ${version}.\n`);
    return version;
  }
  throw new Error(`unknown command: ${args.command ?? "(missing)"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
