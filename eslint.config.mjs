// Test-only ESLint flat config — the only linter wired into CI for now.
//
// Scope: every test file across the workspace. Production code is still
// linted by `tsc --noEmit`; the goal here is to mechanically block the
// assertion-weakening anti-patterns that Phase 1 spent its time fixing
// by hand. See tools/eslint-plugin-aex/index.mjs for the rule bodies
// and tests/cicd remediation history.
//
// Why flat config: ESLint 9 dropped the legacy .eslintrc. Flat config is
// the current shape and works without a build step or a "configs" subdir.
//
// Adding rules: prefer adding them to the local aex plugin so the rule
// body + meta + messages live in one file. Reach for an external plugin
// only when there's a wider community rule we want without re-implementing.

import tseslint from "typescript-eslint";
import aex from "./tools/eslint-plugin-aex/index.mjs";

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
      "**/.wrangler/**",
      "**/coverage/**",
      "**/.cache/**",
      // Skipped/sample / template files committed for offline tests.
      "**/test/fixtures/**",
      // Self-test fixture for the rules themselves — intentionally trips
      // every rule; verified through `pnpm run lint:tests:verify`. Ignored
      // from the normal lint so the main pipeline stays green.
      "tools/eslint-plugin-aex/__verify_rules.test.ts"
    ]
  },
  {
    // Test files only. Production code still relies on `tsc --noEmit` for
    // type errors — adding ESLint there is a much bigger commitment and
    // not in Phase 2 scope.
    files: [
      "**/test/**/*.ts",
      "**/test/**/*.tsx",
      "**/*.test.ts",
      "**/*.test.tsx"
    ],
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
