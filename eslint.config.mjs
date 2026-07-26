// ESLint flat config — two scopes.
//
// 1. TEST FILES get the aex/* assertion-integrity rules: they mechanically block
//    the assertion-weakening anti-patterns that Phase 1 spent its time fixing by
//    hand. See tools/eslint-plugin-aex/index.mjs for the rule bodies.
// 2. ALL SOURCE, production included, gets the module-size budgets. Type errors
//    in production code are still owned by `tsc --noEmit`; the budgets need no
//    type information, so the wider scope costs a parse and nothing else.
//
// Why flat config: ESLint 9 dropped the legacy .eslintrc. Flat config is
// the current shape and works without a build step or a "configs" subdir.
//
// Adding rules: prefer adding them to the local aex plugin so the rule
// body + meta + messages live in one file. Reach for an external plugin
// only when there's a wider community rule we want without re-implementing.

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import tseslint from "typescript-eslint";
import aex from "./tools/eslint-plugin-aex/index.mjs";

const configDir = path.dirname(fileURLToPath(import.meta.url));

/**
 * Every file the module-size budgets parse. Exported so
 * scripts/check-module-size.mjs measures exactly the scope this config lints —
 * a checker with its own copy of the globs is a second authority that drifts.
 */
export const SOURCE_FILES = [
  "**/*.ts",
  "**/*.tsx",
  "**/*.mts",
  "**/*.cts",
  "**/*.js",
  "**/*.jsx",
  "**/*.mjs",
  "**/*.cjs"
];

/** The scope of the aex/* assertion-integrity rules. */
export const TEST_FILES = [
  "**/test/**/*.ts",
  "**/test/**/*.tsx",
  "**/*.test.ts",
  "**/*.test.tsx"
];

/**
 * Test files by every convention this repository actually uses — the scope of the
 * looser line budget. Wider than TEST_FILES on purpose: a `.test.mjs` suite under
 * the source budget would have its `describe()` callbacks measured as functions,
 * which is block nesting, not function size. TEST_FILES is left alone; widening
 * the assertion-integrity rules is a separate change with its own violations to
 * fix.
 */
export const TEST_LINE_BUDGET_FILES = [
  "**/test/**/*.ts",
  "**/test/**/*.tsx",
  "**/test/**/*.mts",
  "**/test/**/*.cts",
  "**/test/**/*.mjs",
  "**/test/**/*.cjs",
  "**/test/**/*.js",
  "**/*.test.ts",
  "**/*.test.tsx",
  "**/*.test.mts",
  "**/*.test.cts",
  "**/*.test.mjs",
  "**/*.test.cjs",
  "**/*.test.js",
  "**/*.spec.ts",
  "**/*.spec.tsx",
  "**/*.spec.mts",
  "**/*.spec.cts",
  "**/*.spec.mjs",
  "**/*.spec.cjs",
  "**/*.spec.js"
];

const SOURCE_LANGUAGE_OPTIONS = {
  parser: tseslint.parser,
  parserOptions: {
    ecmaVersion: 2024,
    sourceType: "module"
  }
};

// Production code carries `// eslint-disable-next-line` comments for rules this
// config does not run (no-console, and similar); without this they would all
// surface as warnings and `--max-warnings=0` would fail on directives that are
// correct elsewhere.
const SOURCE_LINTER_OPTIONS = { reportUnusedDisableDirectives: "off" };

// ---------------------------------------------------------------------------
// MODULE SIZE (review 2026-07-25, E4a)
//
// references/code-standards.md § Bounded modules stated this standard with
// "Enforcement: none today". These are the numbers, and they are a RATCHET, not
// a cleanup: scripts/module-size-baseline.json freezes every file that was over
// budget when the rules landed, and this config relaxes those files to exactly
// their recorded counts. Consequences, all three intended:
//
//   * an unlisted file must comply — a new 700-line module is red here;
//   * a listed file may not grow by one line — its budget IS its current size;
//   * a listed file that SHRINKS goes green here but red in
//     `bun run lint:module-size`, which refuses a stale baseline entry. That is
//     the half a per-file `eslint-disable` can never do: it never forces
//     improvement and never notices a file that got better.
//
// `max-lines-per-function` is the rule that matters — file length is a symptom,
// function length is the defect (code-standards.md says so outright).
//
// `complexity` was CONSIDERED AND REJECTED, measured on this tree 2026-07-25: 44
// violations / 37 files at max 20, 19 / 19 at 30, against the length rules'
// 22 files. It would nearly triple the baseline for offenders the length rules
// already name, and code-standards.md says to judge cohesion, not size — a
// cyclomatic count flags a cohesive wire-schema dispatch as loudly as a tangle.
// Same decision, same reasoning, as platform/eslint.config.mjs.
// ---------------------------------------------------------------------------

/** Budgets. Source/test line counts differ; the function budget does not. */
export const MODULE_SIZE = {
  sourceLines: 600,
  testLines: 800,
  functionLines: 120
};

// Explicit rather than defaulted: a budget whose meaning depends on an ESLint
// default is a budget that changes when ESLint does. Blank lines and comments
// COUNT — a 700-line file is 700 lines to read whatever is on them.
const LINE_COUNT_OPTIONS = { skipBlankLines: false, skipComments: false };

export const MODULE_SIZE_BASELINE_PATH = path.join(
  configDir,
  "scripts",
  "module-size-baseline.json"
);

/**
 * A literal repository path as a flat-config glob. Flat-config `files`/`ignores`
 * are minimatch patterns, and minimatch matches neither a raw nor a
 * backslash-escaped path containing `(` or `[`. Rewriting each metacharacter to
 * `?` — exactly one non-`/` character — is what does match. The residual
 * over-match can only ever relax a path that does not exist, and
 * check-module-size.mjs asserts every baselined path DOES exist.
 */
function asLiteralGlob(repoRelativePath) {
  return repoRelativePath.replace(/[*?[\]{}()!+@]/g, "?");
}

export function loadModuleSizeBaseline() {
  if (!existsSync(MODULE_SIZE_BASELINE_PATH)) return { files: {} };
  return JSON.parse(readFileSync(MODULE_SIZE_BASELINE_PATH, "utf8"));
}

/**
 * The budgets at their real values. Exported so check-module-size.mjs can measure
 * the tree against the strict numbers — the baseline relaxations below are what
 * keep `eslint` green, and the checker must see past them.
 */
export function moduleSizeConfigs() {
  return [
    {
      files: SOURCE_FILES,
      languageOptions: SOURCE_LANGUAGE_OPTIONS,
      linterOptions: SOURCE_LINTER_OPTIONS,
      rules: {
        "max-lines": ["error", { max: MODULE_SIZE.sourceLines, ...LINE_COUNT_OPTIONS }],
        "max-lines-per-function": [
          "error",
          { max: MODULE_SIZE.functionLines, ...LINE_COUNT_OPTIONS, IIFEs: true }
        ]
      }
    },
    {
      // Tests get the looser file budget and NO function budget. `describe()` and
      // `it()` take callbacks, so max-lines-per-function on a test file measures
      // block nesting, not function size: it would flag almost every suite in the
      // repository and the baseline would swallow the signal the rule exists to
      // give. File length still bounds a bloated suite.
      files: TEST_LINE_BUDGET_FILES,
      languageOptions: SOURCE_LANGUAGE_OPTIONS,
      linterOptions: SOURCE_LINTER_OPTIONS,
      rules: {
        "max-lines": ["error", { max: MODULE_SIZE.testLines, ...LINE_COUNT_OPTIONS }],
        "max-lines-per-function": "off"
      }
    }
  ];
}

/**
 * One config object per baselined file, raising that file's budget to exactly its
 * recorded count. Generated, never hand-written: the baseline is the single source
 * of truth and `bun run lint:module-size --write` is the only way to change it.
 */
function moduleSizeBaselineConfigs() {
  return Object.entries(loadModuleSizeBaseline().files ?? {}).map(([file, entry]) => {
    const worstFunction = (entry.functions ?? []).reduce(
      (worst, fn) => Math.max(worst, fn.lines),
      0
    );
    return {
      files: [asLiteralGlob(file)],
      rules: {
        ...(typeof entry.lines === "number"
          ? { "max-lines": ["error", { max: entry.lines, ...LINE_COUNT_OPTIONS }] }
          : {}),
        ...(worstFunction > 0
          ? {
              "max-lines-per-function": [
                "error",
                { max: worstFunction, ...LINE_COUNT_OPTIONS, IIFEs: true }
              ]
            }
          : {})
      }
    };
  });
}

export default tseslint.config(
  // Top-level ignores apply to the whole linter walk (NOT scoped to a
  // single config object). Without this, ESLint descends into generated
  // bundles (`.next/`, `dist/`, `coverage/`) and surfaces failures from
  // inline `/* eslint-disable */` directives in third-party output —
  // pure noise that has nothing to do with our tests.
  {
    ignores: [
      "**/node_modules/**",
      "**/dist/**",
      "**/build/**",
      "**/.next/**",
      "**/coverage/**",
      "**/.cache/**",
      "**/.release-worktrees/**",
      // CI checks out the private platform repo here only for contract parity.
      // It is not public repo source and must not enter public lint scope.
      "_platform/**",
      // Skipped/sample / template files committed for offline tests.
      "**/test/fixtures/**",
      // GENERATED artefacts that happen to live outside `dist/`: the OpenAPI
      // document is emitted from the schemas and the declarations from the
      // document. A module-size budget is a readability rule aimed at code
      // somebody maintains by hand; "split this 3 000-line generated .d.ts" is
      // not an instruction anyone can act on, and baselining it would freeze a
      // number that changes every time a route does.
      "packages/contracts/openapi/**",
      // Self-test fixture for the rules themselves — intentionally trips
      // every rule; verified through `bun run lint:tests:verify`. Ignored
      // from the normal lint so the main pipeline stays green.
      "tools/eslint-plugin-aex/__verify_rules.test.ts"
    ]
  },
  {
    // Test files only. Production code relies on `tsc --noEmit` for type errors;
    // the aex/* rules have no `it`/`expect` to weaken outside a test.
    files: TEST_FILES,
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaVersion: 2024,
        sourceType: "module"
      }
    },
    linterOptions: {
      // `reportUnusedDisableDirectives: false` because production code
      // (referenced indirectly through tsconfig project includes) has
      // `// eslint-disable-next-line` comments for rules we don't run
      // here — those would all surface as warnings otherwise. We only
      // care about the aex/* rule violations in this config.
      reportUnusedDisableDirectives: "off"
    },
    plugins: {
      aex
    },
    rules: {
      // Phase 2 core: block the four anti-pattern families.
      "aex/no-undefined-skip-expect": "error",
      "aex/no-disabled-tests": "error",
      "aex/no-focused-tests": "error",
      "aex/no-conditional-expect": "error"
    }
  },
  // Module size last, and the generated per-file relaxations after the budgets:
  // flat config resolves a rule to the LAST matching object's value, so order is
  // what makes a baselined file's recorded count override the budget. These
  // objects set only the two max-lines rule ids, so they cannot shadow the aex/*
  // rules above.
  ...moduleSizeConfigs(),
  ...moduleSizeBaselineConfigs()
);
