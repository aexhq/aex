#!/usr/bin/env bun
/**
 * C1 — parser ratchet.
 *
 * Counts the surviving hand-rolled allow-list call sites in `@aexhq/contracts`
 * and fails when the count RISES above the committed baseline. Every family
 * ported onto a schema drives it down; it reaches zero when the last one lands,
 * at which point `defineAllowedKeys` / `assertAllowedKeys` are deleted and this
 * check with them.
 *
 * A frozen equality assertion would fail on the way down too, which is what a
 * migration is. A ratchet only forbids the direction that is a regression.
 *
 * Usage:
 *   bun scripts/cicd/check-parser-ratchet.mjs           # check against baseline
 *   bun scripts/cicd/check-parser-ratchet.mjs --write   # lower the baseline
 */
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const baselinePath = resolve(repoRoot, "scripts/cicd/parser-ratchet-baseline.json");

/** Call-site patterns that mark a boundary still validated by hand. */
const PATTERNS = {
  defineAllowedKeys: /defineAllowedKeys</g,
  assertAllowedKeys: /assertAllowedKeys\(/g
};

export function countParserCallSites(files) {
  const counts = Object.fromEntries(Object.keys(PATTERNS).map((key) => [key, 0]));
  for (const file of files) {
    const text = readFileSync(resolve(repoRoot, file), "utf8");
    for (const [name, pattern] of Object.entries(PATTERNS)) {
      counts[name] += text.match(pattern)?.length ?? 0;
    }
  }
  return counts;
}

function main() {
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
  const counts = countParserCallSites(baseline.files);
  const write = process.argv.includes("--write");

  const risen = Object.entries(counts).filter(([name, count]) => count > baseline.maxCallSites[name]);
  if (risen.length > 0) {
    console.error("parser ratchet FAILED — hand-rolled allow-list call sites rose:");
    for (const [name, count] of risen) {
      console.error(`- ${name}: ${count} (baseline ${baseline.maxCallSites[name]})`);
    }
    console.error(
      "\nPort the boundary onto a schema in packages/contracts/src/schemas/ instead of\n" +
        "adding a hand-written allow-list. See references/contract-pipeline-2026-07-25/."
    );
    process.exit(1);
  }

  const lowered = Object.entries(counts).filter(([name, count]) => count < baseline.maxCallSites[name]);
  if (write && lowered.length > 0) {
    writeFileSync(
      baselinePath,
      `${JSON.stringify({ ...baseline, maxCallSites: counts }, null, 2)}\n`,
      "utf8"
    );
    console.log(`parser ratchet baseline lowered: ${JSON.stringify(counts)}`);
    return;
  }

  const total = Object.values(counts).reduce((sum, count) => sum + count, 0);
  if (total === 0) {
    console.log(
      "parser ratchet OK: 0 hand-rolled allow-list call sites remain — " +
        "delete allowed-keys.ts and retire this check."
    );
    return;
  }
  console.log(
    `parser ratchet OK: ${JSON.stringify(counts)} (baseline ${JSON.stringify(baseline.maxCallSites)})` +
      (lowered.length > 0 ? " — run with --write to lower the baseline" : "")
  );
}

if (import.meta.main) {
  main();
}
