#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const baselinePath = resolve(repoRoot, "scripts", "cicd", "public-boundary-baseline.json");
const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
const failures = [];

checkSdkPackageManifest();
checkContractsInlineBaseline();
checkPublicImportDirection();
checkPublicDeploymentClaims();
checkPublicSurfaceLanguage();
checkSdkPackDryRun();

if (failures.length > 0) {
  console.error("public-boundary: failed");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(
  "public-boundary: checked SDK exports/deps, SDK pack dry-run, public import direction, " +
    "public surface language, and public deployment claims."
);
console.log("public-boundary: contracts inline baseline contains only curated public contract modules.");

function checkSdkPackageManifest() {
  const pkgPath = resolve(repoRoot, baseline.sdkPackage.dir, "package.json");
  const pkg = readJson(pkgPath);

  expectEqual("packages/sdk package name", pkg.name, baseline.sdkPackage.name);
  expectEqual("packages/sdk package exports", sortObject(pkg.exports ?? {}), sortObject(baseline.sdkPackage.exports));
  expectEqual("packages/sdk package bin", sortObject(pkg.bin ?? {}), sortObject(baseline.sdkPackage.bin));

  const runtimeDeps = new Set([
    ...Object.keys(pkg.dependencies ?? {}),
    ...Object.keys(pkg.peerDependencies ?? {}),
    ...Object.keys(pkg.optionalDependencies ?? {})
  ]);
  const allowedRuntimeDeps = new Set(baseline.sdkPackage.allowedRuntimeDependencies);
  for (const dep of runtimeDeps) {
    if (dep.startsWith("@antpath/")) {
      failures.push(`packages/sdk publishes runtime dependency ${dep}; inline or move it behind public contracts`);
    }
    if (!allowedRuntimeDeps.has(dep)) {
      failures.push(`packages/sdk runtime dependency ${dep} is not in public-boundary-baseline.json`);
    }
  }
}

function checkContractsInlineBaseline() {
  const contractsIndex = readFileSync(resolve(repoRoot, "packages", "contracts", "src", "index.ts"), "utf8");
  const specifiers = [];
  for (const line of contractsIndex.split(/\r?\n/)) {
    const match = line.match(/^export\s+.*\s+from\s+"([^"]+)";$/);
    if (match) specifiers.push(match[1]);
  }
  expectEqual(
    "packages/contracts src barrel export specifiers",
    [...specifiers].sort(),
    [...baseline.contractsInline.contractsBarrelExportSpecifiers].sort()
  );
}

function checkPublicImportDirection() {
  const publicRoots = baseline.publicSourceRoots.map((root) => resolve(repoRoot, root));
  const allowedAntpathImports = new Set(baseline.temporaryAllowedPublicImports);
  const forbiddenInternalImports = baseline.forbiddenInternalImports;
  const offenders = [];

  for (const root of publicRoots) {
    for (const file of walk(root, isSourceFile)) {
      const text = readFileSync(file, "utf8");
      for (const specifier of importSpecifiers(text)) {
        if (specifier.startsWith("@antpath/") && !allowedAntpathImports.has(specifier)) {
          offenders.push(`${rel(file)} imports ${specifier}`);
          continue;
        }
        if (forbiddenInternalImports.some((name) => specifier === name || specifier.startsWith(`${name}/`))) {
          offenders.push(`${rel(file)} imports workspace package ${specifier}`);
          continue;
        }
        if (specifier.startsWith(".")) {
          const target = resolve(dirname(file), specifier);
          if (!publicRoots.some((rootDir) => isInside(target, rootDir))) {
            offenders.push(`${rel(file)} imports outside public source roots: ${specifier}`);
          }
        }
      }
    }
  }

  if (offenders.length > 0) {
    failures.push(`public source import direction violations:\n${offenders.map((o) => `  ${o}`).join("\n")}`);
  }
}

function checkPublicDeploymentClaims() {
  const docs = publicDocFiles();

  const patterns = [
    { name: "self-host", pattern: /\bself[-\s]?host(?:ed|ing)?\b/i },
    { name: "customer-cloud", pattern: /\bcustomer[-\s]?cloud\b/i },
    { name: "bring-your-own-cloud", pattern: /\bbring\s+your\s+own\s+cloud\b/i }
  ];
  const offenders = [];
  for (const file of docs) {
    const text = readFileSync(file, "utf8");
    const lines = text.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      for (const { name, pattern } of patterns) {
        if (pattern.test(line) && !isUnsupportedDeploymentContext(lines, index)) {
          offenders.push(`${rel(file)}:${index + 1} contains unsupported deployment claim "${name}"`);
        }
      }
    }
  }
  if (offenders.length > 0) {
    failures.push(`public docs advertise unsupported deployment modes:\n${offenders.map((o) => `  ${o}`).join("\n")}`);
  }
}

function isUnsupportedDeploymentContext(lines, index) {
  const window = lines
    .slice(Math.max(0, index - 2), Math.min(lines.length, index + 3))
    .join(" ")
    .toLowerCase();
  return [
    "not as a supported",
    "not a supported",
    "not supported",
    "does not provide",
    "properties of the selected provider",
    "not a custom",
    "unsupported",
    "non-goal",
    "non-goals",
    "do not describe",
    "does not provide",
    "out of scope",
    "not a self-host promise",
    "not a supported self-host deployment claim"
  ].some((needle) => window.includes(needle));
}

function checkPublicSurfaceLanguage() {
  const textFiles = new Map();
  for (const file of publicDocFiles()) {
    textFiles.set(file, "public docs");
  }
  for (const root of baseline.publicSourceRoots) {
    for (const file of walk(resolve(repoRoot, root), isSourceFile)) {
      textFiles.set(file, "public source");
    }
  }

  const forbiddenSurfaceTerms = [
    { name: "Blueprint", pattern: /\bBlueprint\b/ },
    { name: "defineRun", pattern: /\bdefineRun\b/ },
    { name: "compileTemplate", pattern: /\bcompileTemplate\b/ },
    { name: "Template", pattern: /\b[Tt]emplate(?:Definition|ValidationError)?\b|\bTEMPLATE_INVALID\b|\btemplate(?:Name|Hash)\b/ }
  ];
  const riskyClaimTerms = [
    { name: "unqualified cleanup destruction claim", pattern: /\b(?:destroyed|purged)\s+at\s+cleanup\b/i },
    { name: "priority guarantee", pattern: /\bhighest[-\s]?priority\b/i },
    { name: "zero-retention guarantee", pattern: /\bzero[-\s]?retention\b/i }
  ];

  const offenders = [];
  for (const [file, scope] of textFiles) {
    const text = readFileSync(file, "utf8");
    const lines = text.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      for (const { name, pattern } of forbiddenSurfaceTerms) {
        if (pattern.test(line)) {
          offenders.push(`${rel(file)}:${index + 1} (${scope}) contains removed public surface term "${name}"`);
        }
      }
      for (const { name, pattern } of riskyClaimTerms) {
        if (pattern.test(line) && !isUnsupportedDeploymentContext(lines, index)) {
          offenders.push(`${rel(file)}:${index + 1} (${scope}) contains risky public claim "${name}"`);
        }
      }
    }
  }

  if (offenders.length > 0) {
    failures.push(`public surface language violations:\n${offenders.map((o) => `  ${o}`).join("\n")}`);
  }
}

function checkSdkPackDryRun() {
  const pkgDir = resolve(repoRoot, baseline.sdkPackage.dir);
  const result = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: pkgDir,
    encoding: "utf8",
    shell: process.platform === "win32"
  });

  if (result.status !== 0) {
    failures.push(
      `npm pack --dry-run --json --ignore-scripts failed in ${baseline.sdkPackage.dir}:\n` +
        `${result.stderr || result.stdout}`
    );
    return;
  }

  const pack = parseNpmPackJson(result.stdout);
  if (!pack) {
    failures.push("could not parse npm pack --dry-run JSON output for packages/sdk");
    return;
  }

  const files = pack.files.map((file) => normalizePackPath(file.path));
  const allowedPrefixes = baseline.sdkPackage.allowedPackedPathPrefixes;
  const forbiddenPatterns = baseline.sdkPackage.forbiddenPackedPathPatterns.map((pattern) => new RegExp(pattern));
  const pathOffenders = [];
  for (const file of files) {
    if (!allowedPrefixes.some((prefix) => (prefix.endsWith("/") ? file.startsWith(prefix) : file === prefix))) {
      pathOffenders.push(`${file} is outside allowed packed prefixes`);
    }
    if (forbiddenPatterns.some((pattern) => pattern.test(file))) {
      pathOffenders.push(`${file} matches a forbidden private packed path pattern`);
    }
  }
  if (pathOffenders.length > 0) {
    failures.push(`SDK npm pack path leak(s):\n${pathOffenders.map((o) => `  ${o}`).join("\n")}`);
  }

  const missingBuiltFiles = ["dist/index.js", "dist/index.d.ts", "dist/cli.mjs"].filter((required) => !files.includes(required));
  if (missingBuiltFiles.length > 0) {
    failures.push(
      `SDK npm pack dry-run is missing built file(s): ${missingBuiltFiles.join(", ")}. ` +
        "Run pnpm --filter antpath run build before the boundary check."
    );
    return;
  }

  const contractsPrefix = baseline.contractsInline.sdkPackedPrefix;
  const contractsJsModules = files
    .filter((file) => file.startsWith(contractsPrefix) && file.endsWith(".js"))
    .map((file) => `./${file.slice(contractsPrefix.length)}`)
    .sort();
  expectEqual(
    "SDK packed dist/_contracts JavaScript module baseline",
    contractsJsModules,
    [...baseline.contractsInline.allowedModuleFiles].sort()
  );

  const contractsDtsModules = files
    .filter((file) => file.startsWith(contractsPrefix) && file.endsWith(".d.ts"))
    .map((file) => `./${file.slice(contractsPrefix.length).replace(/\.d\.ts$/, ".js")}`)
    .sort();
  expectEqual(
    "SDK packed dist/_contracts declaration module baseline",
    contractsDtsModules,
    [...baseline.contractsInline.allowedModuleFiles].sort()
  );

  const secretOffenders = [];
  const surfaceOffenders = [];
  for (const file of files) {
    if (!isTextPackFile(file)) continue;
    const abs = resolve(pkgDir, file);
    let text;
    try {
      text = readFileSync(abs, "utf8");
    } catch {
      continue;
    }
    for (const { name, pattern } of secretPatterns()) {
      if (pattern.test(text)) secretOffenders.push(`${file} contains ${name}`);
    }
    for (const { name, pattern } of packedSurfacePatterns()) {
      if (pattern.test(text)) surfaceOffenders.push(`${file} contains removed public surface term ${name}`);
    }
  }
  if (secretOffenders.length > 0) {
    failures.push(`SDK packed file secret-shaped content:\n${secretOffenders.map((o) => `  ${o}`).join("\n")}`);
  }
  if (surfaceOffenders.length > 0) {
    failures.push(`SDK packed file public-surface leak(s):\n${surfaceOffenders.map((o) => `  ${o}`).join("\n")}`);
  }
}

function publicDocFiles() {
  const docs = [];
  for (const entry of baseline.publicDocs) {
    const abs = resolve(repoRoot, entry);
    const stat = statSync(abs);
    if (stat.isDirectory()) {
      docs.push(...walk(abs, (file) => file.endsWith(".md")));
    } else {
      docs.push(abs);
    }
  }
  return docs;
}

function importSpecifiers(text) {
  const specs = [];
  const pattern =
    /\bimport\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?["']([^"']+)["']|\bexport\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?["']([^"']+)["']|\bimport\(\s*["']([^"']+)["']\s*\)|\brequire\(\s*["']([^"']+)["']\s*\)/g;
  for (const match of text.matchAll(pattern)) {
    specs.push(match[1] ?? match[2] ?? match[3] ?? match[4]);
  }
  return specs;
}

function walk(root, predicate) {
  const out = [];
  const stack = [root];
  while (stack.length > 0) {
    const dir = stack.pop();
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (entry.name === "node_modules" || entry.name === "dist" || entry.name === "coverage") continue;
      const abs = resolve(dir, entry.name);
      if (entry.isDirectory()) {
        stack.push(abs);
      } else if (entry.isFile() && predicate(abs)) {
        out.push(abs);
      }
    }
  }
  return out.sort();
}

function isSourceFile(file) {
  return /\.(?:cjs|cts|js|jsx|mjs|mts|ts|tsx)$/.test(file);
}

function isTextPackFile(file) {
  return /\.(?:d\.ts|js|json|md|mjs|txt)$/.test(file);
}

function secretPatterns() {
  return [
    { name: "Anthropic API key", pattern: /sk-ant-[A-Za-z0-9_-]{8,}/ },
    { name: "OpenAI API key", pattern: /sk-[A-Za-z0-9_-]{32,}/ },
    { name: "GitHub token", pattern: /gh[pousr]_[A-Za-z0-9_]{20,}/ },
    { name: "private key block", pattern: /-----BEGIN [A-Z ]*PRIVATE KEY-----/ }
  ];
}

function packedSurfacePatterns() {
  return [
    { name: "Blueprint", pattern: /\bBlueprint\b/ },
    { name: "defineRun", pattern: /\bdefineRun\b/ },
    { name: "compileTemplate", pattern: /\bcompileTemplate\b/ },
    { name: "Template", pattern: /\b[Tt]emplate(?:Definition|ValidationError)?\b|\bTEMPLATE_INVALID\b|\btemplate(?:Name|Hash)\b/ }
  ];
}

function parseNpmPackJson(stdout) {
  const trimmed = stdout.trim();
  const start = trimmed.indexOf("[");
  const end = trimmed.lastIndexOf("]");
  if (start < 0 || end < start) return undefined;
  const parsed = JSON.parse(trimmed.slice(start, end + 1));
  return Array.isArray(parsed) ? parsed[0] : undefined;
}

function normalizePackPath(path) {
  const normalized = path.split("\\").join("/");
  return normalized.startsWith("package/") ? normalized.slice("package/".length) : normalized;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function expectEqual(label, actual, expected) {
  const left = JSON.stringify(sortObject(actual));
  const right = JSON.stringify(sortObject(expected));
  if (left !== right) {
    failures.push(`${label} changed.\n  actual: ${left}\n  expected: ${right}`);
  }
}

function sortObject(value) {
  if (Array.isArray(value)) return value.map(sortObject);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, val]) => [key, sortObject(val)]));
}

function isInside(target, root) {
  const relPath = relative(root, target);
  return relPath === "" || (!relPath.startsWith("..") && !relPath.includes(`..${sep}`));
}

function rel(file) {
  return relative(repoRoot, file).split(sep).join("/");
}
