#!/usr/bin/env node
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";

const repoRoot = process.cwd();
const lower = "ant" + "path";
const title = "Ant" + "path";
const upper = "ANT" + "PATH";
const oldBrand = new RegExp(
  `${lower}|${title}|${upper}|@${lower}|x-${lower}|${lower}\\.ai`
);
const ignoredDirs = new Set([
  ".git",
  "node_modules",
  "dist",
  "coverage",
  ".next",
  ".generated",
  ".source",
  ".wrangler",
  "out",
  "playwright-report",
  "test-results",
  ".turbo",
  ".vercel"
]);
const ignoredBinaryExts = /\.(png|jpe?g|gif|webp|ico|pdf|zip|tgz|gz|woff2?|ttf|otf|wasm|sqlite|db)$/i;

const failures = [];

function shouldSkipFile(name) {
  if (name.endsWith(".tsbuildinfo")) return true;
  if (name.startsWith(".env") && name !== ".env.example") return true;
  return ignoredBinaryExts.test(name);
}

function posix(path) {
  return path.split("\\").join("/");
}

function scanPath(absPath) {
  const rel = posix(relative(repoRoot, absPath));
  if (oldBrand.test(rel)) {
    failures.push(`${rel}: path contains old brand`);
  }
}

function scanFile(absPath) {
  const rel = posix(relative(repoRoot, absPath));
  const bytes = readFileSync(absPath);
  if (bytes.includes(0)) return;
  const text = bytes.toString("utf8");
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    if (oldBrand.test(lines[i])) {
      failures.push(`${rel}:${i + 1}: contains old brand`);
    }
  }
}

function walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory() && ignoredDirs.has(entry.name)) continue;
    const abs = join(dir, entry.name);
    scanPath(abs);
    if (entry.isDirectory()) {
      walk(abs);
    } else if (entry.isFile() && !shouldSkipFile(entry.name)) {
      scanFile(abs);
    }
  }
}

walk(repoRoot);

if (failures.length > 0) {
  process.stderr.write(`Old brand references found:\n${failures.join("\n")}\n`);
  process.exit(1);
}

process.stdout.write("old-brand scan passed\n");
