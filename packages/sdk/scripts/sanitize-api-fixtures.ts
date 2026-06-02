/**
 * `pnpm --filter antpath run fixtures:sanitize` — the SECURITY-CRITICAL,
 * fully-offline half of record-replay.
 *
 * Takes the gitignored RAW recordings (`test/fixtures/api-recordings/*.raw.json`,
 * which may contain real secrets) → strips every secret-SHAPED value via the
 * shared value-agnostic redactor → normalizes ids/timestamps → writes the
 * committed sanitized fixtures (`*.sanitized.json`).
 *
 * It FAILS (exit non-zero) if ANY produced fixture still contains a
 * secret-shaped value (`containsSecretLikeValue`). It ALSO re-scans every
 * already-committed `*.sanitized.json` so a fixture that was committed before
 * the redactor learned a new shape is caught on the next run — a sanitized
 * fixture that still carries a secret must never sit in the tree.
 *
 * Pure logic lives in lib/fixtures.ts (unit-tested offline); this file is just
 * the file I/O + argv shell.
 *
 * Usage:
 *   tsx scripts/sanitize-api-fixtures.ts            # sanitize raw → sanitized + verify
 *   tsx scripts/sanitize-api-fixtures.ts --verify   # ONLY re-scan committed fixtures
 *   tsx scripts/sanitize-api-fixtures.ts --help
 */

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildSanitizedFixture,
  findResidualSecrets,
  parseSanitizedFixture,
  ResidualSecretError,
  type RawRecording
} from "./lib/fixtures.js";

// Overridable so the exit-code contract can be exercised against a scratch dir
// in tests; defaults to the committed fixtures dir for real runs.
const FIXTURES_DIR =
  process.env.ANTPATH_FIXTURES_DIR ?? fileURLToPath(new URL("../test/fixtures/api-recordings/", import.meta.url));
const RAW_SUFFIX = ".raw.json";
const SANITIZED_SUFFIX = ".sanitized.json";

function listFixtures(suffix: string): string[] {
  let entries: string[];
  try {
    entries = readdirSync(FIXTURES_DIR);
  } catch {
    return [];
  }
  return entries.filter((e) => e.endsWith(suffix)).sort();
}

function help(): void {
  process.stdout.write(
    [
      "fixtures:sanitize — strip secret-shaped values from raw Anthropic recordings.",
      "",
      "  (no args)   sanitize every test/fixtures/api-recordings/*.raw.json into",
      "              *.sanitized.json, then verify NO sanitized fixture carries a secret.",
      "  --verify    only re-scan committed *.sanitized.json (no write); CI safety net.",
      "  --help      this message.",
      "",
      "Exits non-zero if any sanitized fixture still contains a secret-shaped value.",
      `Fixtures dir: ${FIXTURES_DIR}`
    ].join("\n") + "\n"
  );
}

/** Re-scan all committed sanitized fixtures for residual secrets. Returns the
 * number of offending files (0 = clean). */
function verifyCommitted(): number {
  let bad = 0;
  for (const name of listFixtures(SANITIZED_SUFFIX)) {
    const path = join(FIXTURES_DIR, name);
    let parsed: unknown;
    try {
      parsed = JSON.parse(readFileSync(path, "utf8"));
    } catch (err) {
      process.stderr.write(`  [BAD] ${name}: not valid JSON (${(err as Error).message})\n`);
      bad++;
      continue;
    }
    const fixture = parseSanitizedFixture(parsed);
    const residual = findResidualSecrets(fixture.events);
    if (residual.length > 0) {
      bad++;
      process.stderr.write(
        `  [SECRET] ${name}: ${residual.length} residual secret-shaped value(s): ` +
          residual.map((r) => `${r.path} (${r.sample})`).join(", ") +
          "\n"
      );
    } else {
      process.stdout.write(`  [OK] ${name} (${fixture.events.length} events, no secrets)\n`);
    }
  }
  return bad;
}

function main(): void {
  const args = process.argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    help();
    return;
  }
  const verifyOnly = args.includes("--verify");

  if (!verifyOnly) {
    const raws = listFixtures(RAW_SUFFIX);
    if (raws.length === 0) {
      process.stdout.write(
        "no *.raw.json recordings found — nothing to sanitize. " +
          "Produce one with `pnpm --filter antpath run fixtures:record:anthropic` (live, creds-gated).\n"
      );
    }
    for (const name of raws) {
      const rawPath = join(FIXTURES_DIR, name);
      const raw = JSON.parse(readFileSync(rawPath, "utf8")) as RawRecording;
      try {
        const fixture = buildSanitizedFixture(raw);
        const outName = name.slice(0, -RAW_SUFFIX.length) + SANITIZED_SUFFIX;
        writeFileSync(join(FIXTURES_DIR, outName), JSON.stringify(fixture, null, 2) + "\n", "utf8");
        process.stdout.write(`  [SANITIZED] ${name} -> ${outName} (${fixture.events.length} events)\n`);
      } catch (err) {
        if (err instanceof ResidualSecretError) {
          // Do NOT write the offending output — fail loud so the secret never
          // reaches the tree.
          process.stderr.write(`  [SECRET] ${name}: ${err.message}\n`);
          process.exitCode = 1;
          return;
        }
        throw err;
      }
    }
  }

  // Always re-scan committed fixtures (the CI safety net): a fixture that
  // passed an OLD redactor but trips a newly-learned shape is caught here.
  const bad = verifyCommitted();
  if (bad > 0) {
    process.stderr.write(`fixtures:sanitize FAILED — ${bad} sanitized fixture(s) still contain a secret.\n`);
    process.exitCode = 1;
    return;
  }
  process.stdout.write("fixtures:sanitize OK — no secret-shaped values in any committed fixture.\n");
}

main();
