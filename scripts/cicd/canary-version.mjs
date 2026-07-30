/**
 * The canary version scheme, and the one place that knows its shape.
 *
 * WHY IT LOOKS LIKE THIS. The previous scheme validated the source SHA and then
 * threw it away, returning a bare `<major>.<minor>.<patch>-canary`. Every push
 * at a given base therefore resolved the SAME string, and npm refuses to
 * republish a version it has already served — so the SECOND green push at any
 * base was always red. Measured on the registry on 2026-07-28: `0.46.3-canary`,
 * `0.46.4-canary` and `0.46.5-canary` are all published, which is the record of
 * that failure being "fixed" three times by bumping the base version. Bumping
 * the base buys exactly one more push. `module-canary.mjs` now routes three
 * packages through this function, so the same bug would have cost three reds a
 * push instead of one.
 *
 * The fix is to put the identity back into the version:
 *
 *     0.46.4-canary.19876543210.gb1c3d5f
 *     ^base          ^run id      ^source sha, 7 hex
 *
 * Both suffix components earn their place:
 *
 *   - `<sha7>` is what makes the version UNIQUE per source tree. Two pushes at
 *     one base are two different commits, therefore two different versions.
 *   - `<run>` is what makes the version ORDERED. Semver compares dot-separated
 *     prerelease identifiers left to right, numeric ones numerically; a bare
 *     `g<sha7>` would sort lexically, i.e. arbitrarily, so `npm view` and any
 *     range resolution would pick a random canary as "the highest". The run id
 *     is monotonic for the lifetime of the repository and is already recorded
 *     as `workflowRunId` in the canary manifest, so the version and the
 *     manifest name the same run. It is stable across re-runs of one run, which
 *     is what keeps a re-run from minting a second version of identical bytes.
 *
 * Pre-launch there is no canary-channel consumer to break, which is why the
 * SCHEME changes rather than the base version chasing free numbers.
 * `applySdkVersion` here and `moduleSourceTag` in `module-canary.mjs` both
 * refuse anything that is not this shape; both read the shape from
 * {@link isCanaryVersion} so it is stated once.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER_CORE_RE = /^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const SOURCE_SHA_RE = /^[0-9a-f]{40}$/;
/** A semver numeric identifier: no leading zeros, so precedence stays numeric. */
const RUN_RE = /^(?:0|[1-9]\d*)$/;

/**
 * The full canary shape. Anchored, and the ONLY declaration of it — every other
 * gate in the release path asks {@link isCanaryVersion} rather than restating it.
 */
export const CANARY_VERSION_RE = /^\d+\.\d+\.\d+-canary\.(?:0|[1-9]\d*)\.g[0-9a-f]{7}$/;

/** @param {unknown} version */
export function isCanaryVersion(version) {
  return typeof version === "string" && CANARY_VERSION_RE.test(version);
}

/**
 * Resolve the immutable canary version for one module at one commit.
 *
 * @param {{ baseVersion?: unknown, sha?: unknown, run?: unknown }} options
 * @returns {string}
 */
export function buildCanaryVersion({ baseVersion, sha, run }) {
  const match = SEMVER_CORE_RE.exec(String(baseVersion ?? ""));
  if (!match) throw new Error(`invalid base version: ${baseVersion ?? "(missing)"}`);

  const normalizedSha = String(sha ?? "").toLowerCase();
  if (!SOURCE_SHA_RE.test(normalizedSha)) {
    throw new Error(`invalid source SHA: ${sha ?? "(missing)"}`);
  }

  // Required, not defaulted. A default would silently resolve two commits to one
  // version again on any caller that forgot to pass it — the exact failure this
  // function exists to close.
  const normalizedRun = String(run ?? "").trim();
  if (!RUN_RE.test(normalizedRun)) {
    throw new Error(
      `invalid workflow run id: ${run ?? "(missing)"} — expected a decimal integer without leading zeros`
    );
  }

  return `${match[1]}.${match[2]}.${match[3]}-canary.${normalizedRun}.g${normalizedSha.slice(0, 7)}`;
}

export function applySdkVersion(repoRoot, version) {
  if (!isCanaryVersion(version)) {
    throw new Error(`refusing to apply invalid canary version: ${version}`);
  }

  const packagePath = resolve(repoRoot, "packages", "sdk", "package.json");
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
  if (packageJson.name !== "@aexhq/sdk") {
    throw new Error(`unexpected package at ${packagePath}: ${packageJson.name ?? "(missing name)"}`);
  }
  packageJson.version = version;
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, "utf8");
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
      sha: required(args, "sha"),
      run: required(args, "run")
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
