#!/usr/bin/env bun
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const staticSpecifierPattern =
  /\bimport\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?["']([^"']+)["']|\bexport\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?["']([^"']+)["']|\bimport\(\s*["']([^"']+)["']\s*\)/g;

export function checkGeneratedDistClosure(distDir) {
  const root = resolve(distDir);
  const failures = [];
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    return [`generated dist directory does not exist: ${root}`];
  }

  for (const file of walk(root).filter((entry) => entry.endsWith(".js"))) {
    const dts = declarationPathForJs(file);
    if (!existsSync(dts)) {
      failures.push(`${rel(root, file)} is missing declaration ${rel(root, dts)}`);
    }

    const text = readFileSync(file, "utf8");
    for (const specifier of staticRelativeSpecifiers(text)) {
      const target = resolve(dirname(file), specifier);
      if (!isInside(target, root)) {
        failures.push(`${rel(root, file)} imports outside generated dist: ${specifier}`);
        continue;
      }
      if (!specifier.endsWith(".js")) {
        failures.push(`${rel(root, file)} uses non-JS relative specifier: ${specifier}`);
        continue;
      }
      if (!existsSync(target)) {
        failures.push(`${rel(root, file)} references missing module ${specifier}`);
        continue;
      }
      const targetDts = declarationPathForJs(target);
      if (!existsSync(targetDts)) {
        failures.push(`${rel(root, file)} references ${specifier} but ${rel(root, targetDts)} is missing`);
      }
    }
  }

  return failures.sort();
}

export function formatGeneratedDistClosureFailures(distDir, failures) {
  return [
    `generated-dist closure check failed for ${resolve(distDir)}`,
    ...failures.map((failure) => `- ${failure}`)
  ].join("\n");
}

function staticRelativeSpecifiers(text) {
  const specs = [];
  for (const match of text.matchAll(staticSpecifierPattern)) {
    const specifier = match[1] ?? match[2] ?? match[3];
    if (specifier?.startsWith(".")) specs.push(specifier);
  }
  return specs;
}

function declarationPathForJs(file) {
  return file.replace(/\.js$/, ".d.ts");
}

function walk(root) {
  const out = [];
  const stack = [root];
  while (stack.length > 0) {
    const dir = stack.pop();
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const abs = resolve(dir, entry.name);
      if (entry.isDirectory()) {
        stack.push(abs);
      } else if (entry.isFile()) {
        out.push(abs);
      }
    }
  }
  return out.sort();
}

function isInside(target, root) {
  const relPath = relative(root, target);
  return relPath === "" || (!relPath.startsWith("..") && !isAbsolute(relPath));
}

function rel(root, file) {
  return relative(root, file).split(sep).join("/");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const distDir = process.argv[2];
  if (!distDir) {
    console.error("usage: bun scripts/cicd/check-generated-dist-closure.mjs <dist-dir>");
    process.exit(2);
  }

  const failures = checkGeneratedDistClosure(distDir);
  if (failures.length > 0) {
    console.error(formatGeneratedDistClosureFailures(distDir, failures));
    process.exit(1);
  }
  console.log(`generated-dist closure OK: ${resolve(distDir)}`);
}
