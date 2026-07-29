// ESLint flat config — one scope.
//
// TEST FILES get the aex/* assertion-integrity rules: they mechanically block
// the assertion-weakening anti-patterns that Phase 1 spent its time fixing by
// hand. See tools/eslint-plugin-aex/index.mjs for the rule bodies. Type errors
// in production code are owned by `tsc --noEmit`.
//
// There were once module-size budgets here (`max-lines`,
// `max-lines-per-function`) with a frozen per-file ratchet. Removed 2026-07-28:
// the ratchet froze each file at its own size, so every planned integration of
// parallel branches summed their growths and reded the gate on work that was
// individually honest. The workspace-root `references/code-standards.md`
// § Bounded modules remains the standard; it is reviewed, not enforced.
//
// Why flat config: ESLint 9 dropped the legacy .eslintrc. Flat config is
// the current shape and works without a build step or a "configs" subdir.
//
// Adding rules: prefer adding them to the local aex plugin so the rule
// body + meta + messages live in one file. Reach for an external plugin
// only when there's a wider community rule we want without re-implementing.

import tseslint from "typescript-eslint";
import aex from "./tools/eslint-plugin-aex/index.mjs";

/** The scope of the aex/* assertion-integrity rules. */
export const TEST_FILES = [
  "**/test/**/*.ts",
  "**/test/**/*.tsx",
  "**/*.test.ts",
  "**/*.test.tsx"
];

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
      // Skipped/sample / template files committed for offline tests.
      "**/test/fixtures/**",
      // GENERATED artefacts that happen to live outside `dist/`: the OpenAPI
      // document is emitted from the schemas and the declarations from the
      // document. Nothing here is maintained by hand, so no lint verdict on it
      // is actionable.
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
  }
);
