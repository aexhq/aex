import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE_SHA_RE = /^[0-9a-f]{40}$/;

export function applyReleaseSource(repoRoot, sourceSha) {
  const normalizedSourceSha = String(sourceSha ?? "").toLowerCase();
  if (!SOURCE_SHA_RE.test(normalizedSourceSha)) {
    throw new Error(`invalid release source SHA: ${sourceSha ?? "(missing)"}`);
  }

  const packagePath = resolve(repoRoot, "packages", "sdk", "package.json");
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
  if (packageJson.name !== "@aexhq/sdk") {
    throw new Error(`unexpected package at ${packagePath}: ${packageJson.name ?? "(missing name)"}`);
  }

  packageJson.aexRelease = { sourceSha: normalizedSourceSha };
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, "utf8");
  return normalizedSourceSha;
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command };
  for (let index = 0; index < rest.length; index += 1) {
    const key = rest[index];
    if (!key?.startsWith("--")) throw new Error(`unexpected argument: ${key ?? "(missing)"}`);
    const value = rest[++index];
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
  if (args.command !== "apply") throw new Error(`unknown command: ${args.command ?? "(missing)"}`);
  const sourceSha = applyReleaseSource(resolve(args.repoRoot || process.cwd()), required(args, "sha"));
  process.stdout.write(`Bound SDK package metadata to release source ${sourceSha}.\n`);
  return sourceSha;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
