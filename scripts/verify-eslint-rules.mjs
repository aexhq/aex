// Verify the aex ESLint rules still catch what they're meant to.
//
// Two independent verifications, because a rule can go dead in two different
// ways:
//
//  1. RULE BODIES -- lints `tools/eslint-plugin-aex/__verify_rules.test.ts`
//     (which intentionally trips every rule) and checks that the exact expected
//     violation set fires. A drift -- a rule stops catching its pattern, or starts
//     catching something else -- fails this script. It supplies its own config.
//  2. RULE WIRING -- reads the ACTUAL `eslint.config.mjs` back for a real test
//     path, so a rule that is silently switched off, mis-scoped, or shadowed by
//     flat config's last-object-wins resolution fails here. Verification 1
//     cannot see that class of regression: it supplies its own config.

import { ESLint } from "eslint";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

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
// Verification 2: the rules are ENABLED by the shipped config.
//
// Verification 1 supplies its own config, so it proves the rule bodies work and
// nothing about whether `eslint.config.mjs` still switches them on for a real
// test path. A rule deleted from the config, mis-scoped, or shadowed by flat
// config's last-object-wins resolution is invisible above and caught here.
// Severity 2 = error.
// ---------------------------------------------------------------------------

const EXPECTED_TEST_FILE_SEVERITIES = {
  "aex/no-undefined-skip-expect": 2,
  "aex/no-disabled-tests": 2,
  "aex/no-focused-tests": 2,
  "aex/no-conditional-expect": 2
};

const realConfigEslint = new ESLint({ cwd: root });
const configFailures = [];
const testFileConfig = await realConfigEslint.calculateConfigForFile(
  path.join(root, "packages", "sdk", "test", "__verify_rules_wiring.test.ts")
);
for (const [ruleId, severity] of Object.entries(EXPECTED_TEST_FILE_SEVERITIES)) {
  const configured = testFileConfig.rules?.[ruleId];
  const actual = Array.isArray(configured) ? configured[0] : configured;
  if (actual !== severity) {
    configFailures.push(
      `${ruleId} is not enabled for test files: expected severity ${severity}, got ${String(actual)}`
    );
  }
}

if (configFailures.length > 0) {
  console.error("Rule-verification FAILED (real eslint.config.mjs):");
  for (const failure of configFailures) console.error(`  ${failure}`);
  process.exit(1);
}

console.log(
  `Rule-verification OK: ${messages.length} violations on fixture, matching ${EXPECTED.length} expected rules; ` +
    `${Object.keys(EXPECTED_TEST_FILE_SEVERITIES).length} rule(s) verified enabled by the real config.`
);
