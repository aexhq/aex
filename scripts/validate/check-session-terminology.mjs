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
  "scripts/validate/check-session-terminology.mjs",
  "scripts/validate/check-session-terminology.test.mjs"
];

const forbidden = [
  ["legacy env/table name", /\b(?:LIST_RUNS_PUBLIC|RUNS_TABLE|WORKSPACE_MAX_CONCURRENT_RUNS|AEX_WORKSPACE_MAX_CONCURRENT_RUNS)\b/],
  ["legacy public quota field", /\bmaxConcurrentRuns\b/],
  ["legacy runs route", /["'`]\/(?:api\/)?runs(?:[/?#][^"'`]*)?["'`]/],
  ["legacy CLI command", /\baex run\b/],
  ["legacy list-runs API", /\blistRuns\b|\blist_runs\b|\bno_orphan_child_runs\b/],
  [
    "mechanical session rewrite",
    /\bsessionning\b|\bsessionner\b|\bsession\/session\b|\baex starttime\b|\b(?:still|loop|handler|erase|tool|brain|characters|sequences) sessions\b|\bSessions an?\b|\bsessions (?:a|an) (?:command|turn|watchdog)\b/i
  ]
];

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
