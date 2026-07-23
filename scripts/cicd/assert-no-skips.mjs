#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { collectJunitTests, isJunitReportText } from "./junit-report.mjs";

// `--name-pattern <regex>` scopes both the skip check and the zero-tests check
// to test full names matching the regex. Needed for `bun test -t`-filtered
// lanes: bun marks every NON-selected test `<skipped/>` in the junit report, so
// an unscoped read of a filtered run would false-fail. Semantics/implementation
// mirror the platform repository's copy of this script — change both together.
const args = process.argv.slice(2);
/** @type {string | undefined} */
let namePatternRaw;
/** @type {string[]} */
const positionals = [];
for (let i = 0; i < args.length; i += 1) {
  const arg = args[i];
  if (arg === "--name-pattern") {
    const value = args[i + 1];
    if (!value) {
      console.error("[assert-no-skips] --name-pattern requires a regex value.");
      process.exit(1);
    }
    namePatternRaw = value;
    i += 1;
    continue;
  }
  positionals.push(arg);
}
if (positionals.length !== 1) {
  console.error("usage: assert-no-skips.mjs [--name-pattern <regex>] <json-or-junit-report>");
  process.exit(1);
}
const reportPath = positionals[0];

/** @type {RegExp | undefined} */
let namePattern;
if (namePatternRaw !== undefined) {
  try {
    namePattern = new RegExp(namePatternRaw);
  } catch (error) {
    console.error(
      `[assert-no-skips] invalid --name-pattern ${JSON.stringify(namePatternRaw)}: ${error instanceof Error ? error.message : String(error)}`
    );
    process.exit(1);
  }
}
const scopedSuffix = namePatternRaw === undefined ? "" : ` matching --name-pattern ${JSON.stringify(namePatternRaw)}`;

if (!existsSync(reportPath)) {
  // bun's junit reporter writes NO outfile at all when zero tests are
  // collected, so a vanished report IS the zero-coverage case — hard failure.
  console.error(`[assert-no-skips] report not found: ${reportPath}`);
  process.exit(1);
}

const findings = [];
let totalTests = 0;

// Shape is detected from CONTENT, never the filename: JUnit XML (bun test
// --reporter=junit / node --test --test-reporter=junit) vs JSON (Vitest
// jest-style or Playwright).
const rawReport = readFileSync(reportPath, "utf8");
if (isJunitReportText(rawReport)) {
  try {
    const collected = collectJunitTests(rawReport, { namePattern });
    totalTests += collected.total;
    for (const entry of collected.skipped) {
      findings.push(`${entry.name} (${entry.status})`);
    }
  } catch (error) {
    console.error(`[assert-no-skips] failed to parse ${reportPath}: ${error.message}`);
    process.exit(1);
  }
} else {
  let report;
  try {
    report = JSON.parse(rawReport);
  } catch (error) {
    console.error(`[assert-no-skips] failed to parse ${reportPath}: ${error.message}`);
    process.exit(1);
  }

  collectVitest(report);
  collectPlaywright(report);

  // Under a name pattern the scoped per-assertion count is authoritative; the
  // unscoped summary counter would defeat the zero-tests check for patterns
  // that match nothing.
  if (namePattern === undefined && typeof report.numTotalTests === "number") {
    totalTests = Math.max(totalTests, report.numTotalTests);
  }
}

if (totalTests === 0) {
  console.error(`[assert-no-skips] ${reportPath} reported zero tests${scopedSuffix}.`);
  process.exit(1);
}

if (findings.length > 0) {
  console.error(`[assert-no-skips] ${reportPath} reported skipped/disabled tests:`);
  for (const finding of findings.slice(0, 50)) {
    console.error(`  - ${finding}`);
  }
  if (findings.length > 50) {
    console.error(`  ... and ${findings.length - 50} more`);
  }
  process.exit(1);
}

console.log(`[assert-no-skips] OK: ${totalTests} tests${scopedSuffix}, no skipped/disabled entries.`);

function collectVitest(node) {
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node.testResults)) {
    for (const suite of node.testResults) {
      const suiteName = typeof suite.name === "string" ? suite.name : "";
      for (const result of suite.assertionResults ?? []) {
        const status = String(result.status ?? "").toLowerCase();
        const title = vitestTitle(suiteName, result);
        // Match on the jest/vitest full name (mirrors the platform copy and the
        // junit branch, which both scope on the reporter's full test name).
        const matchName = result.fullName ?? result.title ?? title;
        if (namePattern && !namePattern.test(matchName)) continue;
        totalTests += 1;
        if (isDisabledStatus(status)) findings.push(`${title} (${status})`);
      }
    }
  }
}

function collectPlaywright(node, ancestors = []) {
  if (!node || typeof node !== "object") return;
  const nextAncestors = typeof node.title === "string" && node.title.length > 0 ? [...ancestors, node.title] : ancestors;
  if (Array.isArray(node.specs)) {
    for (const spec of node.specs) {
      const specTitle = typeof spec.title === "string" ? spec.title : "<unnamed spec>";
      for (const test of spec.tests ?? []) {
        totalTests += 1;
        const title = [...nextAncestors, specTitle].join(" > ");
        const status = String(test.status ?? test.expectedStatus ?? "").toLowerCase();
        const expectedStatus = String(test.expectedStatus ?? "").toLowerCase();
        const resultStatuses = (test.results ?? []).map((result) => String(result.status ?? "").toLowerCase());
        const annotations = (test.annotations ?? []).map((annotation) => String(annotation.type ?? "").toLowerCase());
        if (
          isDisabledStatus(status) ||
          isDisabledStatus(expectedStatus) ||
          resultStatuses.some(isDisabledStatus) ||
          annotations.includes("skip")
        ) {
          findings.push(`${title} (skipped)`);
        }
      }
    }
  }
  for (const suite of node.suites ?? []) {
    collectPlaywright(suite, nextAncestors);
  }
}

function vitestTitle(suiteName, result) {
  const parts = [];
  if (suiteName) parts.push(suiteName);
  for (const ancestor of result.ancestorTitles ?? []) {
    if (ancestor) parts.push(ancestor);
  }
  if (result.title) parts.push(result.title);
  return parts.join(" > ") || "<unnamed test>";
}

function isDisabledStatus(status) {
  return status === "skip" || status === "skipped" || status === "pending" || status === "todo" || status === "disabled";
}
