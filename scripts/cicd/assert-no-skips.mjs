#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";

const reportPath = process.argv[2];
if (!reportPath) {
  console.error("usage: assert-no-skips.mjs <json-report>");
  process.exit(1);
}
if (!existsSync(reportPath)) {
  console.error(`[assert-no-skips] report not found: ${reportPath}`);
  process.exit(1);
}

let report;
try {
  report = JSON.parse(readFileSync(reportPath, "utf8"));
} catch (error) {
  console.error(`[assert-no-skips] failed to parse ${reportPath}: ${error.message}`);
  process.exit(1);
}

const findings = [];
let totalTests = 0;

collectVitest(report);
collectPlaywright(report);

if (typeof report.numTotalTests === "number") {
  totalTests = Math.max(totalTests, report.numTotalTests);
}

if (totalTests === 0) {
  console.error(`[assert-no-skips] ${reportPath} reported zero tests.`);
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

console.log(`[assert-no-skips] OK: ${totalTests} tests, no skipped/disabled entries.`);

function collectVitest(node) {
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node.testResults)) {
    for (const suite of node.testResults) {
      const suiteName = typeof suite.name === "string" ? suite.name : "";
      for (const result of suite.assertionResults ?? []) {
        const status = String(result.status ?? "").toLowerCase();
        const title = vitestTitle(suiteName, result);
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
