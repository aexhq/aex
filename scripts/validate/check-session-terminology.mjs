#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const skippedPathParts = [
  "references/",
  "node_modules/",
  "dist/",
  "coverage/",
  ".next/",
  "release-diagnostics/",
  "vercel-build-context/",
  ".tmp-gh-run-",
  ".tmp-gh-actions-",
  "bun.lock",
  "scripts/validate/check-session-terminology.mjs"
];

const forbidden = [
  ["legacy env/table name", /\b(?:LIST_RUNS_PUBLIC|RUNS_TABLE|WORKSPACE_MAX_CONCURRENT_RUNS|AEX_WORKSPACE_MAX_CONCURRENT_RUNS)\b/],
  ["legacy public quota field", /\bmaxConcurrentRuns\b/],
  ["legacy session id spelling", /\brunId\b|\brun_id\b|\brun-id\b/],
  ["legacy runs route", /\/runs\b|\bruns\//],
  ["legacy CLI command", /\baex run\b/],
  ["legacy artifact/config/event filename", /\brun-events\b|\brun\.json\b|\brun-config\b/],
  ["legacy product event/metric", /\baex\.run\b|\brun\.settled\b|\brun\.total_ms\b/],
  ["legacy audit/session object field", /\btargetType:\s*["']run["']|\btarget:\s*\{\s*type:\s*["']run["']|\bmanifest\.run\b|\bresult\.run\b|\brunBody\b|\brunErrorMessage\b|\brunFinishedCount\b|\brunDump\b|\brunOutputCell\b/],
  ["legacy snake/kebab product term", /\brun-[a-z0-9-]+|\brun_[a-z0-9_]+/],
  ["legacy exported product name", /\b[A-Za-z0-9_]*Runs[A-Z][A-Za-z0-9_]*\b|\b[A-Za-z0-9_]*Run[A-Z][A-Za-z0-9_]*\b/],
  ["legacy tool/invariant name", /\blist_runs\b|\bno_orphan_child_runs\b/],
  ["legacy screaming env prefix", /\bRUN_[A-Z0-9_]*\b/],
  [
    "legacy standalone product phrase",
    /\brun (?:row|rows|record|records|artifact|artifacts|config|cost|custody|retention|unit|trace|lifecycle|limits|status|execution|orchestrator|bus|submission|compute|capacity|teardown|provider|deliverable|deliverables|polling|readiness|finished|events|files|secret|secrets|vault|flow|actions|honesty|delete|children|error|did not fail|launched|time)\b/
  ]
];

const allowedExternalLine = (line) =>
  /\bruns-on\b|github\.run_id|github\.run_attempt|GITHUB_RUN_(?:ID|ATTEMPT)|actions\/runs\b|workflow_runs\b|\bgh run\b/.test(line) ||
  /\bRunTask\b|\brunTask\.sync\b|\becs:runTask\b|\becs:RunTask\b|\bEcsRunTask\b/.test(line) ||
  /\bbun run\b/.test(line);

const git = spawnSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { encoding: "buffer" });
if (git.status !== 0) {
  process.stderr.write(git.stderr.toString("utf8"));
  process.exit(git.status ?? 1);
}

const files = git.stdout
  .toString("utf8")
  .split("\0")
  .filter(Boolean)
  .filter((file) => !skippedPathParts.some((part) => file.includes(part)));

const violations = [];
for (const file of files) {
  let buf;
  try {
    buf = readFileSync(file);
  } catch {
    continue;
  }
  if (buf.includes(0)) continue;
  const text = buf.toString("utf8");
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (allowedExternalLine(line)) continue;
    for (const [label, pattern] of forbidden) {
      if (pattern.test(line)) violations.push(`${file}:${i + 1}: ${label}: ${line.trim()}`);
    }
  }
}

if (violations.length > 0) {
  process.stderr.write(`Found legacy run terminology outside references/:\n${violations.join("\n")}\n`);
  process.exit(1);
}

process.stdout.write("session terminology check passed\n");
