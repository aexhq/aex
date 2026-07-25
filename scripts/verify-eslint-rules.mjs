// Verify the aex ESLint rules still catch what they're meant to.
//
// Two independent verifications, because the two rule families fail in different
// ways:
//
//  1. RULE BODIES -- lints `tools/eslint-plugin-aex/__verify_rules.test.ts`
//     (which intentionally trips every rule) and checks that the exact expected
//     violation set fires. A drift -- a rule stops catching its pattern, or starts
//     catching something else -- fails this script. It supplies its own config.
//  2. RULE WIRING -- lints synthetic sources at real repository paths through the
//     ACTUAL `eslint.config.mjs`, so a module-size budget that is silently
//     switched off, mis-scoped, or shadowed by flat config's last-object-wins rule
//     resolution fails here. Verification 1 cannot see that class of regression.

import { ESLint } from "eslint";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { MODULE_SIZE, loadModuleSizeBaseline, moduleSizeConfigs } from "../eslint.config.mjs";

const __filename = fileURLToPath(import.meta.url);
const root = path.resolve(path.dirname(__filename), "..");
const fixture = path.join(root, "tools", "eslint-plugin-aex", "__verify_rules.test.ts");

const EXPECTED = [
  { ruleId: "aex/no-undefined-skip-expect", count: 1 },
  { ruleId: "aex/no-conditional-expect", count: 2 },
  // skip + skipIf + bun-only `.if` + bun-only `.todoIf` (`.failing` is
  // deliberately allowed — it asserts failure, it does not skip).
  { ruleId: "aex/no-disabled-tests", count: 4 },
  { ruleId: "aex/no-focused-tests", count: 1 }
];

const eslint = new ESLint({
  cwd: root,
  overrideConfigFile: true,
  overrideConfig: [
    {
      files: ["**/*.test.ts"],
      languageOptions: {
        parser: (await import("typescript-eslint")).default.parser,
        parserOptions: { ecmaVersion: 2024, sourceType: "module" }
      },
      plugins: {
        aex: (await import(pathToFileURL(path.join(root, "tools", "eslint-plugin-aex", "index.mjs")).href)).default
      },
      rules: {
        "aex/no-undefined-skip-expect": "error",
        "aex/no-disabled-tests": "error",
        "aex/no-focused-tests": "error",
        "aex/no-conditional-expect": "error"
      }
    }
  ]
});

const results = await eslint.lintFiles([fixture]);

if (results.length !== 1) {
  console.error(`expected exactly 1 lint result, got ${results.length}`);
  process.exit(1);
}

const messages = results[0].messages;
const counts = new Map();
for (const message of messages) {
  counts.set(message.ruleId, (counts.get(message.ruleId) ?? 0) + 1);
}

const failures = [];
for (const { ruleId, count } of EXPECTED) {
  const actual = counts.get(ruleId) ?? 0;
  if (actual !== count) {
    failures.push(`  ${ruleId}: expected ${count}, got ${actual}`);
  }
}
for (const [ruleId, actual] of counts) {
  if (!EXPECTED.some((expected) => expected.ruleId === ruleId)) {
    failures.push(`  unexpected rule ${ruleId} fired ${actual} times`);
  }
}

if (failures.length > 0) {
  console.error("Rule-verification FAILED:");
  for (const failure of failures) console.error(failure);
  console.error("\nActual messages:");
  for (const message of messages) {
    console.error(`  ${message.line}:${message.column} ${message.ruleId} - ${message.message}`);
  }
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Verification 2: the module-size budgets, through the real config (E4a).
//
// The ratchet in scripts/check-module-size.mjs proves the CURRENT tree matches the
// baseline. It cannot prove the budgets are still switched on: if the rules were
// removed from eslint.config.mjs, the measured violation set would go empty and
// the ratchet's own "no longer a violation" assertion would fire -- but only
// because every file went quiet at once, which reads like a mass fix.
// ---------------------------------------------------------------------------

/** `n` lines of trivial statements -- a file with no function in it. */
function fileOf(lines) {
  return `${Array.from({ length: lines }, (_, index) => `export const v${index} = ${index};`).join("\n")}\n`;
}

/** One function whose BODY is `lines` long, in a file far under the file budget. */
function functionOf(lines) {
  const body = Array.from({ length: lines }, (_, index) => `  const v${index} = ${index};`).join("\n");
  return `export function big() {\n${body}\n  return 0;\n}\n`;
}

const MODULE_SIZE_CASES = [
  {
    label: `a ${MODULE_SIZE.sourceLines + 100}-line source file is over the ${MODULE_SIZE.sourceLines}-line budget`,
    filePath: "packages/sdk/src/__verify_module_size.ts",
    source: fileOf(MODULE_SIZE.sourceLines + 100),
    expected: ["max-lines"]
  },
  {
    // The rule that matters. Same directory as the baselined client.ts, so a
    // baseline glob that over-matches its neighbours fails here.
    label: `a ${MODULE_SIZE.functionLines + 10}-line function is over the ${MODULE_SIZE.functionLines}-line budget`,
    filePath: "packages/sdk/src/__verify_module_size.ts",
    source: functionOf(MODULE_SIZE.functionLines + 10),
    expected: ["max-lines-per-function"]
  },
  {
    label: "the budgets reach the CLI and the contracts package too",
    filePath: "packages/contracts/src/__verify_module_size.ts",
    source: functionOf(MODULE_SIZE.functionLines + 10),
    expected: ["max-lines-per-function"]
  },
  {
    label: `a ${MODULE_SIZE.testLines - 100}-line test file is within the ${MODULE_SIZE.testLines}-line test budget`,
    filePath: "apps/user-tests/test/__verify_module_size.test.ts",
    source: fileOf(MODULE_SIZE.testLines - 100),
    expected: []
  },
  {
    label: `a ${MODULE_SIZE.testLines + 10}-line test file is over the ${MODULE_SIZE.testLines}-line test budget`,
    filePath: "apps/user-tests/test/__verify_module_size.test.ts",
    source: fileOf(MODULE_SIZE.testLines + 10),
    expected: ["max-lines"]
  },
  {
    // Deliberate: describe()/it() take callbacks, so the function budget on a test
    // file measures block nesting rather than function size.
    label: "the function budget is off for test files",
    filePath: "apps/user-tests/test/__verify_module_size.test.ts",
    source: functionOf(MODULE_SIZE.functionLines + 10),
    expected: []
  },
  {
    label: "a .test.mjs suite gets the test budget, not the source budget",
    filePath: "scripts/validate/__verify_module_size.test.mjs",
    source: functionOf(MODULE_SIZE.functionLines + 10),
    expected: []
  }
];

/**
 * Which line budget every test-file convention resolves to.
 *
 * This is the regression that shipped once in the platform repository and must not
 * ship here: the config listed `**​/*.test.ts` and nothing else, so `.test.mjs`
 * suites were measured against the 600-line PRODUCTION budget with
 * `max-lines-per-function` on, which measures their `describe()` callbacks.
 * Baselining them would have frozen test files under the production budget
 * permanently. Every convention this repository uses is asserted here, plus the
 * ones it does not use yet, so adding one is not a silent drop to the source budget.
 */
const BUDGET_CLASSIFICATION = [
  ["packages/sdk/src/client.ts", MODULE_SIZE.sourceLines],
  ["scripts/cicd/check-public-boundary.mjs", MODULE_SIZE.sourceLines],
  ["packages/cli/test/host.test.ts", MODULE_SIZE.testLines],
  ["apps/user-tests/test/live/edge-event-stream.user.test.ts", MODULE_SIZE.testLines],
  ["apps/user-tests/scripts/user-bun-test.test.mjs", MODULE_SIZE.testLines],
  ["packages/sdk/test/helper.mjs", MODULE_SIZE.testLines],
  ["packages/sdk/src/thing.test.mts", MODULE_SIZE.testLines],
  ["packages/sdk/src/thing.test.js", MODULE_SIZE.testLines],
  ["packages/sdk/src/thing.test.cjs", MODULE_SIZE.testLines],
  ["apps/user-tests/scripts/smoke.spec.ts", MODULE_SIZE.testLines]
];

const realConfigEslint = new ESLint({ cwd: root });
const configFailures = [];

// Measured through a second ESLint instance carrying the STRICT budgets (the same
// overrideConfig check-module-size.mjs uses), because the shipped config relaxes
// baselined files to their recorded counts and those would mask the budget here.
const strictEslint = new ESLint({ cwd: root, overrideConfig: moduleSizeConfigs() });
for (const [file, expectedMax] of BUDGET_CLASSIFICATION) {
  const fileConfig = await strictEslint.calculateConfigForFile(path.join(root, file));
  const configured = fileConfig.rules?.["max-lines"];
  const actual = Array.isArray(configured) ? configured[1]?.max : undefined;
  if (actual !== expectedMax) {
    const label = expectedMax === MODULE_SIZE.testLines ? "test" : "source";
    configFailures.push(
      `${file} must be measured against the ${label} budget (${expectedMax}), got ${String(actual)}`
    );
  }
}

for (const testCase of MODULE_SIZE_CASES) {
  const caseResults = await realConfigEslint.lintText(testCase.source, {
    filePath: path.join(root, testCase.filePath),
    // A path that matches no config object is a legitimate outcome; it must not be
    // reported as a warning, which `--max-warnings=0` would treat as a failure.
    warnIgnored: false
  });
  const actual = (caseResults[0]?.messages ?? []).map((message) => message.ruleId).sort();
  const expected = [...testCase.expected].sort();
  if (actual.join(",") !== expected.join(",")) {
    configFailures.push(
      `${testCase.label} (${testCase.filePath}): expected [${expected.join(", ")}], got [${actual.join(", ")}]`
    );
  }
}

// The baseline relaxations are generated as flat-config `files` globs, and a
// literal path containing `(` or `[` matches neither raw nor backslash-escaped, so
// eslint.config.mjs rewrites those characters to `?`. Verify the rewrite still
// lands on the file it names: a glob that silently matches nothing would make
// `eslint` red (loud), but one that silently matches too much would make it weak
// (quiet). Only the metacharacter paths are checked; the rest are literal.
for (const [file, entry] of Object.entries(loadModuleSizeBaseline().files ?? {})) {
  if (!/[*?[\]{}()!+@]/.test(file)) continue;
  const fileConfig = await realConfigEslint.calculateConfigForFile(path.join(root, file));
  const expectations = [
    ["max-lines", entry.lines],
    [
      "max-lines-per-function",
      (entry.functions ?? []).reduce((worst, fn) => Math.max(worst, fn.lines), 0)
    ]
  ];
  for (const [ruleId, recorded] of expectations) {
    if (typeof recorded !== "number" || recorded === 0) continue;
    const configured = fileConfig.rules?.[ruleId];
    const actual = Array.isArray(configured) ? configured[1]?.max : undefined;
    if (actual !== recorded) {
      configFailures.push(
        `baseline relaxation does not reach ${file}: ${ruleId} max is ${String(actual)}, baseline records ${recorded}`
      );
    }
  }
}

if (configFailures.length > 0) {
  console.error("Rule-verification FAILED (real eslint.config.mjs):");
  for (const failure of configFailures) console.error(`  ${failure}`);
  process.exit(1);
}

console.log(
  `Rule-verification OK: ${messages.length} violations on fixture, matching ${EXPECTED.length} expected rules; ` +
    `${MODULE_SIZE_CASES.length} module-size case(s) and ${BUDGET_CLASSIFICATION.length} ` +
    "budget-classification case(s) verified against the real config."
);
