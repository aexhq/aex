// Verify the aex ESLint rules still catch what they're meant to.
//
// Sessions ESLint programmatically against `tools/eslint-plugin-aex/__verify_rules.test.ts`
// (which intentionally trips every rule) and checks that the exact expected
// violation set fires. A drift -- a rule stops catching its pattern, or starts
// catching something else -- fails this script.

import { ESLint } from "eslint";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const root = path.resolve(path.dirname(__filename), "..");
const fixture = path.join(root, "tools", "eslint-plugin-aex", "__verify_rules.test.ts");

const EXPECTED = [
  { ruleId: "aex/no-undefined-skip-expect", count: 1 },
  { ruleId: "aex/no-conditional-expect", count: 2 },
  { ruleId: "aex/no-disabled-tests", count: 2 },
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

console.log(`Rule-verification OK: ${messages.length} violations on fixture, matching ${EXPECTED.length} expected rules.`);
