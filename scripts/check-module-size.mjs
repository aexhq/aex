#!/usr/bin/env node
/**
 * The module-size ratchet (review 2026-07-25, E4a).
 *
 * `eslint.config.mjs` carries the budgets — `max-lines` 600 (source) / 800
 * (test) and `max-lines-per-function` 120 — and relaxes every file listed in
 * scripts/module-size-baseline.json to exactly its recorded count. That makes
 * plain `eslint` refuse a new violation and refuse growth in a listed file. It
 * cannot do the other half, and the other half is what makes this a ratchet
 * rather than a suppression list:
 *
 *   (i)   no violation outside the baseline            — eslint does this too;
 *   (ii)  each baselined count may only DECREASE, and a decrease must be
 *         recorded — so a file that got better cannot silently keep its old
 *         allowance and cannot silently re-enter later;
 *   (iii) every baselined path still exists on disk    — so deleting a file
 *         forces the baseline to be cleaned up instead of accumulating ghosts.
 *
 * This is deliberately the same shape as scripts/docs/docs-baseline.json and its
 * ratchet assertions in the workspace root repository. Same failure mode, same
 * remedy, same words.
 *
 * platform/scripts/check-module-size.mjs and aex/scripts/check-module-size.mjs are
 * kept BYTE-IDENTICAL. The two repositories are separate git remotes with no
 * shared package — the same reason tools/eslint-plugin-aex is duplicated. Change
 * one, copy it to the other; everything repository-specific lives in
 * eslint.config.mjs, which this file only imports from.
 *
 * Usage:
 *   bun scripts/check-module-size.mjs            # the gate (wired into `lint`)
 *   bun scripts/check-module-size.mjs --write    # record a shrink or a deletion
 *
 * `--write` REFUSES to record a growth or a newly-violating file. There is no
 * flag to override that: re-freezing a regression is a hand edit to the JSON,
 * where it shows up in review as a number going up.
 */
import { ESLint } from "eslint";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  MODULE_SIZE,
  MODULE_SIZE_BASELINE_PATH,
  SOURCE_FILES,
  TEST_LINE_BUDGET_FILES,
  moduleSizeConfigs
} from "../eslint.config.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const FROZEN = "2026-07-25";

const BASELINE_NOTE =
  "Frozen over-budget set for the module-size budgets in eslint.config.mjs. " +
  "check-module-size.mjs refuses a violation absent from this file, refuses a " +
  "recorded count that is no longer reached, and refuses an entry whose file no " +
  "longer exists. It can only shrink. Record a fix with " +
  "`bun run lint:module-size --write`.";

/**
 * Runs ESLint over the tree with the budgets forced back to their strict values.
 *
 * `overrideConfig` is APPENDED to the real eslint.config.mjs, so the repository's
 * own `ignores` and file scope still apply and the generated baseline relaxations
 * are overridden rather than duplicated here. That is the point: the checker must
 * measure exactly what the config lints, or the two drift.
 *
 * @returns {Promise<{
 *   violations: Map<string, {kind: "source"|"test", lines?: number, functions: {label: string, lines: number}[]}>,
 *   unmeasurable: string[]
 * }>}
 */
async function measure() {
  const eslint = new ESLint({ cwd: repoRoot, overrideConfig: moduleSizeConfigs() });
  const results = await eslint.lintFiles(["."]);

  /** @type {Map<string, {kind: "source"|"test", lines?: number, functions: {label: string, lines: number}[]}>} */
  const violations = new Map();
  /** Files ESLint could not parse. NEVER skipped silently — see main(). */
  const unmeasurable = [];

  for (const result of results) {
    const file = path.relative(repoRoot, result.filePath).split(path.sep).join("/");
    for (const message of result.messages) {
      if (message.fatal === true) {
        // A file that cannot be parsed cannot be measured, so the budgets are
        // unenforced there. Recording it as "no violations" would silently omit it
        // from the baseline and hand it an unlimited allowance.
        unmeasurable.push(`${file}:${message.line ?? 0}:${message.column ?? 0} ${message.message}`);
        continue;
      }
      if (message.ruleId !== "max-lines" && message.ruleId !== "max-lines-per-function") continue;

      const count = Number(/\((\d+)\)/.exec(message.message)?.[1]);
      if (!Number.isInteger(count)) {
        // Fail fast: the count is parsed out of ESLint's message, so an upstream
        // rewording must break loudly rather than silently record zeros.
        throw new Error(
          `cannot read a line count out of ${message.ruleId} message: ${JSON.stringify(message.message)}`
        );
      }

      const entry = violations.get(file) ?? { functions: [] };
      if (message.ruleId === "max-lines") entry.lines = count;
      else entry.functions.push({ label: labelOf(message.message), lines: count });
      violations.set(file, entry);
    }
  }

  for (const [file, entry] of violations) {
    entry.functions.sort((a, b) => b.lines - a.lines || a.label.localeCompare(b.label));
    entry.kind = await budgetKindOf(eslint, file);
  }
  return { violations, unmeasurable };
}

/**
 * Which budget the config actually applied to this path.
 *
 * Asked of ESLint rather than answered by a second copy of the test-file globs.
 * A hand-written predicate here is exactly how `**​/*.test.mjs` came to be measured
 * as production code: eslint.config.mjs listed one set of test globs and the
 * checker another. TEST_LINE_BUDGET_FILES is now the only authority, and this
 * function reads the decision back out of the resolved config.
 *
 * @returns {Promise<"source"|"test">}
 */
async function budgetKindOf(eslint, file) {
  const config = await eslint.calculateConfigForFile(path.resolve(repoRoot, file));
  const configured = config.rules?.["max-lines"];
  const max = Array.isArray(configured) ? configured[1]?.max : undefined;
  if (max === MODULE_SIZE.testLines) return "test";
  if (max === MODULE_SIZE.sourceLines) return "source";
  // The two budgets must stay distinct for this to be answerable at all.
  throw new Error(
    `cannot tell which budget applies to ${file}: resolved max-lines max is ${String(max)}, ` +
      `expected ${MODULE_SIZE.sourceLines} (source) or ${MODULE_SIZE.testLines} (test)`
  );
}

/** "Function 'submit' has too many lines (799)…" -> "Function 'submit'". */
function labelOf(message) {
  const label = message.split(" has too many lines")[0];
  return label === message ? "function" : label;
}

function budgetFor(kind) {
  return kind === "test" ? MODULE_SIZE.testLines : MODULE_SIZE.sourceLines;
}

function reasonFor(file, entry) {
  const parts = [];
  if (typeof entry.lines === "number") {
    parts.push(`${entry.lines} lines (budget ${budgetFor(entry.kind)})`);
  }
  if (entry.functions.length > 0) {
    const worst = entry.functions[0];
    parts.push(
      `${entry.functions.length} function(s) over ${MODULE_SIZE.functionLines} lines, worst ${worst.label} at ${worst.lines}`
    );
  }
  return `frozen ${FROZEN}: ${parts.join("; ")}`;
}

function loadBaseline() {
  if (!existsSync(MODULE_SIZE_BASELINE_PATH)) return null;
  return JSON.parse(readFileSync(MODULE_SIZE_BASELINE_PATH, "utf8"));
}

/**
 * The three ratchet assertions, plus the reason requirement the docs baseline
 * also carries. Returns a list of human-readable failures, empty when green.
 */
function ratchetFailures(baseline, current) {
  const failures = [];
  const recordedFiles = Object.entries(baseline.files ?? {});

  // (i) nothing new.
  for (const [file, entry] of current) {
    const recorded = baseline.files?.[file];
    if (recorded === undefined) {
      failures.push(
        `NEW over-budget file, not in the baseline: ${file} — ${reasonFor(file, entry)}. ` +
          "Split it along a seam the domain already has; the baseline is closed to new entries."
      );
    }
  }

  for (const [file, recorded] of recordedFiles) {
    // (iii) every baselined path still exists.
    if (!existsSync(path.resolve(repoRoot, file))) {
      failures.push(
        `baselined file no longer exists: ${file}. Remove the entry: bun run lint:module-size --write`
      );
      continue;
    }

    const actual = current.get(file);
    if (actual === undefined) {
      failures.push(
        `baselined file is no longer over budget: ${file}. Record the fix: bun run lint:module-size --write`
      );
      continue;
    }

    // (ii) file line count may only decrease, and a decrease must be recorded.
    if (typeof recorded.lines === "number" || typeof actual.lines === "number") {
      const before = recorded.lines ?? budgetFor(actual.kind);
      const after = actual.lines ?? 0;
      if (after > before) {
        failures.push(
          `${file}: grew to ${after} lines; the baseline froze it at ${before}. A listed file may only shrink.`
        );
      } else if (after < before) {
        failures.push(
          `${file}: shrank to ${after} lines from ${before} — good. Record it: bun run lint:module-size --write`
        );
      }
    }

    // (ii) again, per over-budget function. Compared as a descending sequence
    // rather than by name: renaming a function must not read as a fix, and two
    // anonymous callbacks in one file have the same label.
    const recordedFunctions = [...(recorded.functions ?? [])].sort((a, b) => b.lines - a.lines);
    if (actual.functions.length > recordedFunctions.length) {
      const extra = actual.functions[recordedFunctions.length];
      failures.push(
        `${file}: a NEW function is over the ${MODULE_SIZE.functionLines}-line budget — ` +
          `${extra.label} at ${extra.lines} lines. The baseline froze ${recordedFunctions.length}.`
      );
    } else if (actual.functions.length < recordedFunctions.length) {
      failures.push(
        `${file}: ${recordedFunctions.length - actual.functions.length} baselined over-budget ` +
          "function(s) are now within budget — good. Record it: bun run lint:module-size --write"
      );
    }
    for (let index = 0; index < Math.min(actual.functions.length, recordedFunctions.length); index += 1) {
      const after = actual.functions[index];
      const before = recordedFunctions[index];
      if (after.lines > before.lines) {
        failures.push(
          `${file}: ${after.label} grew to ${after.lines} lines; the baseline froze ${before.lines}.`
        );
      } else if (after.lines < before.lines) {
        failures.push(
          `${file}: ${after.label} shrank to ${after.lines} lines from ${before.lines} — good. ` +
            "Record it: bun run lint:module-size --write"
        );
      }
    }

    if (typeof recorded.reason !== "string" || recorded.reason.trim().length < 10) {
      failures.push(`${file}: baseline entry carries no reason.`);
    }
  }

  return failures;
}

function serialize(current) {
  const files = {};
  for (const file of [...current.keys()].sort((a, b) => a.localeCompare(b))) {
    const entry = current.get(file);
    files[file] = {
      kind: entry.kind,
      ...(typeof entry.lines === "number" ? { lines: entry.lines } : {}),
      ...(entry.functions.length > 0 ? { functions: entry.functions } : {}),
      reason: reasonFor(file, entry)
    };
  }
  const overBudgetFunctions = [...current.values()].reduce(
    (total, entry) => total + entry.functions.length,
    0
  );
  return {
    generated: FROZEN,
    note: BASELINE_NOTE,
    budgets: {
      maxLinesSource: MODULE_SIZE.sourceLines,
      maxLinesTest: MODULE_SIZE.testLines,
      maxLinesPerFunction: MODULE_SIZE.functionLines,
      scope: {
        source: SOURCE_FILES,
        test: TEST_LINE_BUDGET_FILES
      }
    },
    counts: { files: current.size, overBudgetFunctions },
    files
  };
}

/** @returns {Promise<number>} process exit code. */
export async function main(argv = process.argv.slice(2)) {
  const write = argv.includes("--write");
  const { violations: current, unmeasurable } = await measure();
  const baseline = loadBaseline();

  // Reported, never swallowed — and reported ALONGSIDE the ratchet verdict rather
  // than instead of it, so one unparseable file does not hide whether the rest of
  // the tree is clean. It still fails the gate, and it still blocks --write: a
  // baseline generated while a file was unparseable would omit that file and hand
  // it an unlimited allowance, with nothing downstream to say so.
  if (unmeasurable.length > 0) {
    console.error(
      `Module-size gate cannot measure ${unmeasurable.length} file(s) — the budgets are UNENFORCED there:`
    );
    for (const problem of unmeasurable) console.error(`  ${problem}`);
    console.error(
      "A file that does not parse cannot be measured. Fix the syntax error; " +
        "this gate will not silently exclude it.\n"
    );
  }

  if (write) {
    if (unmeasurable.length > 0) return 1;
    if (baseline !== null) {
      const regressions = ratchetFailures(baseline, current).filter(
        (failure) => failure.includes("grew to") || failure.includes("NEW ")
      );
      if (regressions.length > 0) {
        console.error(
          "--write refuses to record a regression. The baseline is a ratchet:\n" +
            regressions.map((failure) => `  ${failure}`).join("\n")
        );
        return 1;
      }
    }
    writeFileSync(MODULE_SIZE_BASELINE_PATH, `${JSON.stringify(serialize(current), null, 2)}\n`, "utf8");
    const functions = [...current.values()].reduce((total, entry) => total + entry.functions.length, 0);
    console.log(
      `module-size-baseline.json: ${current.size} file(s), ${functions} over-budget function(s)`
    );
    return 0;
  }

  if (baseline === null) {
    console.error(
      `missing ${path.relative(repoRoot, MODULE_SIZE_BASELINE_PATH)}. ` +
        "Bootstrap it once with: bun run lint:module-size --write"
    );
    return 1;
  }

  const failures = ratchetFailures(baseline, current);
  if (failures.length > 0) {
    console.error(`Module-size ratchet FAILED (${failures.length} problem(s)):`);
    for (const failure of failures) console.error(`  ${failure}`);
    console.error(
      `\nBudgets: max-lines ${MODULE_SIZE.sourceLines} source / ${MODULE_SIZE.testLines} test, ` +
        `max-lines-per-function ${MODULE_SIZE.functionLines} (source only). ` +
        "See references/code-standards.md § Bounded modules."
    );
    return 1;
  }

  const functions = [...current.values()].reduce((total, entry) => total + entry.functions.length, 0);
  console.log(
    `Module-size ratchet OK: ${current.size} baselined file(s), ${functions} over-budget function(s), ` +
      "none new, none grown, none stale, all present."
  );
  return unmeasurable.length > 0 ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main();
}
