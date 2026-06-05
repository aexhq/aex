#!/usr/bin/env node
/**
 * Cross-repo contract parity gate.
 *
 * The public `@aexhq/contracts` source is a published mirror of the platform
 * contract surface (`aexhq/aex-platform` `packages/contracts/src`, itself a
 * mirror of `packages/shared/src`). When the two drift silently, published
 * SDK/CLI clients ship a different wire contract than the Worker enforces —
 * the class of bug this gate exists to catch at PR time instead of in prod.
 *
 * Two axes are enforced:
 *   1. platform/contracts/src/<f>  <->  public/contracts/src/<f>
 *      (the published-surface parity). Every overlapping source file is
 *      compared line-multiset after CRLF normalisation. A line present on
 *      exactly one side is a divergence.
 *   2. the SSRF host deny-list, whose single source of truth is
 *      platform/shared/src/blueprint.ts. The public copy lives inline in
 *      run-config.ts (public has no blueprint.ts), so the three deny
 *      functions are compared directly so the Wave-1 hardening can't
 *      regress on one side only.
 *
 * A divergence fails the gate UNLESS it is recorded in the baseline
 * (`contract-parity-baseline.json`, same dir). The baseline is the explicit
 * allowlist: it pins KNOWN, intentional or tracked-temporary deltas
 * (public-only `createSkillBundleDirect`, the `./blueprint.js` vs
 * `./run-config.js` import-path mirror, public's de-branded doc comments, and
 * any platform feature not yet ported). Each baseline entry carries a `why`.
 * NEW divergence that isn't in the baseline fails — that's the tripwire.
 *
 * The gate SKIPS (exit 0, loud notice) when the platform tree isn't checked
 * out, so public-only CI and forks without the platform PAT still pass; it
 * only enforces when both trees are present.
 *
 * Run `node scripts/cicd/check-contract-parity.mjs --update` after an
 * intentional, reviewed divergence to refresh the baseline.
 */
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const publicRoot = resolve(here, "..", ".."); // public/
const baselinePath = join(here, "contract-parity-baseline.json");
const UPDATE = process.argv.includes("--update");

function findPlatformRoot() {
  const candidates = process.env.AEX_PLATFORM_DIR
    ? [resolve(process.env.AEX_PLATFORM_DIR)]
    : [
        resolve(publicRoot, "..", "aex-platform"), // CI: sibling checkout of aexhq/aex-platform
        resolve(publicRoot, "..", "platform"), // workspace: <root>/platform + <root>/public
      ];
  for (const c of candidates) {
    // Require both trees the gate reads, so a misconfigured override skips
    // (loud notice) rather than crashing mid-read.
    if (
      existsSync(join(c, "packages", "contracts", "src")) &&
      existsSync(join(c, "packages", "shared", "src", "blueprint.ts"))
    ) {
      return c;
    }
  }
  return null;
}

const platformRoot = findPlatformRoot();
if (!platformRoot) {
  process.stdout.write(
    "contract-parity: platform tree not found (set AEX_PLATFORM_DIR to enforce) — SKIPPED\n"
  );
  process.exit(0);
}

const platformContractsSrc = join(platformRoot, "packages", "contracts", "src");
const publicContractsSrc = join(publicRoot, "packages", "contracts", "src");
const blueprintPath = join(platformRoot, "packages", "shared", "src", "blueprint.ts");
const publicRunConfigPath = join(publicContractsSrc, "run-config.ts");

function norm(text) {
  return text.replace(/\r/g, "");
}
function readNorm(path) {
  return norm(readFileSync(path, "utf8"));
}

/**
 * The SSRF deny functions are checked on axis 2 against blueprint.ts (the
 * SoT), NOT against platform/contracts (whose copy is the pre-Wave-1
 * monolith). Drop the deny region from the axis-1 multiset on both sides so
 * the same lines aren't double-reported under run-config.ts.
 */
function stripDenyRegion(text) {
  // Anchor on the deny-function DEFINITIONS (the `function denyReason…`
  // declarations), not their call sites — `denyReasonForMcpHost` is also
  // referenced earlier inside the parser. The block runs from the doc-comment
  // of the first definition to the next `function ` declaration after the last
  // definition. Present in both the hardened (public, split) and pre-Wave-1
  // monolithic (platform/contracts) copies, so the region excises
  // symmetrically.
  const defRe = /^(?:export )?function denyReason[A-Za-z0-9]*\(/m;
  const firstMatch = defRe.exec(text);
  if (!firstMatch) return text;
  const firstFn = firstMatch.index;
  const docStart = text.lastIndexOf("/**", firstFn);
  // End at the first `function ` declaration that follows the LAST deny def.
  let lastDefEnd = firstFn;
  for (const m of text.matchAll(/^(?:export )?function denyReason[A-Za-z0-9]*\(/gm)) {
    lastDefEnd = m.index;
  }
  const afterRe = /^(?:export )?function (?!denyReason)/m;
  const tail = text.slice(lastDefEnd);
  const afterMatch = afterRe.exec(tail);
  if (!afterMatch) return text;
  const end = lastDefEnd + afterMatch.index;
  return text.slice(0, docStart === -1 ? firstFn : docStart) + text.slice(end);
}

/**
 * Order-insensitive line-multiset diff: a line counted more times on one side
 * than the other is a divergence on that side. Intentional: parity cares
 * WHETHER a contract line exists on both sides, not where it sits.
 */
function diffLines(aText, bText) {
  const count = new Map();
  for (const l of aText.split("\n")) count.set(l, (count.get(l) ?? 0) + 1);
  for (const l of bText.split("\n")) count.set(l, (count.get(l) ?? 0) - 1);
  const divergences = [];
  for (const [line, n] of count) {
    if (line.trim() === "") continue; // blank lines never carry contract meaning
    const side = n > 0 ? "platform" : "public";
    for (let i = 0; i < Math.abs(n); i++) divergences.push({ side, line: line.trim() });
  }
  return divergences;
}

/** Stable fingerprint for a divergence, used as the baseline key. */
function fp(scope, side, line) {
  return `${scope}|${side}|${line}`;
}

// Collect every current divergence across both axes.
const found = new Map(); // fp -> { scope, side, line }

// ---- Axis 1: platform/contracts <-> public/contracts -----------------------
const publicFiles = readdirSync(publicContractsSrc).filter((f) => f.endsWith(".ts"));
for (const file of publicFiles) {
  const platformFile = join(platformContractsSrc, file);
  if (!existsSync(platformFile)) {
    const entry = { scope: file, side: "public", line: "<file present in public, absent in platform/contracts>" };
    found.set(fp(entry.scope, entry.side, entry.line), entry);
    continue;
  }
  let platformText = readNorm(platformFile);
  let publicText = readNorm(join(publicContractsSrc, file));
  if (file === "run-config.ts") {
    platformText = stripDenyRegion(platformText);
    publicText = stripDenyRegion(publicText);
  }
  for (const d of diffLines(platformText, publicText)) {
    const entry = { scope: file, side: d.side, line: d.line };
    found.set(fp(file, d.side, d.line), entry);
  }
}

// ---- Axis 2: SSRF deny-list (blueprint.ts <-> run-config.ts) ----------------
function extractDenyBlock(text) {
  const start = text.indexOf("denyReasonForHostIp");
  const end = text.indexOf("parseRemoteMcpTransport", start);
  if (start === -1 || end === -1) return null;
  const docStart = text.lastIndexOf("/**", start);
  return text
    .slice(docStart === -1 ? start : docStart, end)
    .replace(/\bexport function\b/g, "function") // public keeps these local (intentional surface delta)
    .replace(/^\s*\*\s*Surface tracked by.*$/gm, "") // internal-doc ref strip
    .trim();
}
const platformDeny = extractDenyBlock(readNorm(blueprintPath));
const publicDeny = extractDenyBlock(readNorm(publicRunConfigPath));
if (platformDeny === null || publicDeny === null) {
  const entry = { scope: "deny-list", side: "n/a", line: "could not locate deny functions in one tree" };
  found.set(fp(entry.scope, entry.side, entry.line), entry);
} else {
  for (const d of diffLines(platformDeny, publicDeny)) {
    const entry = { scope: "deny-list", side: d.side, line: d.line };
    found.set(fp("deny-list", d.side, d.line), entry);
  }
}

// ---- Baseline reconcile ----------------------------------------------------
let baseline = { entries: [] };
if (existsSync(baselinePath)) {
  baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
}
const baselineKeys = new Set(baseline.entries.map((e) => fp(e.scope, e.side, e.line)));

if (UPDATE) {
  const merged = [...found.values()]
    .map((e) => {
      const existing = baseline.entries.find((b) => fp(b.scope, b.side, b.line) === fp(e.scope, e.side, e.line));
      return { ...e, why: existing?.why ?? "TODO: explain this divergence" };
    })
    .sort((a, b) => fp(a.scope, a.side, a.line).localeCompare(fp(b.scope, b.side, b.line)));
  writeFileSync(baselinePath, JSON.stringify({ entries: merged }, null, 2) + "\n");
  process.stdout.write(`contract-parity: baseline updated (${merged.length} entries)\n`);
  process.exit(0);
}

const unexplained = [...found.values()].filter((e) => !baselineKeys.has(fp(e.scope, e.side, e.line)));
const stale = baseline.entries.filter((e) => !found.has(fp(e.scope, e.side, e.line)));

if (unexplained.length > 0) {
  process.stderr.write(
    `contract-parity FAILED: ${unexplained.length} NEW divergence${unexplained.length === 1 ? "" : "s"} not in the baseline:\n` +
      unexplained.map((e) => `  - [${e.scope}] (${e.side}-only) ${e.line.slice(0, 140)}`).join("\n") +
      "\n\nPort the platform change into public, or — if the divergence is " +
      "intentional public-only surface or tracked-temporary — run " +
      "`node scripts/cicd/check-contract-parity.mjs --update` and fill in the " +
      "`why` for each new baseline entry.\n"
  );
  process.exit(1);
}

if (stale.length > 0) {
  // Stale baseline entries mean a recorded divergence was resolved (good!) —
  // fail so the baseline is trimmed and can't accumulate dead allowances that
  // would mask a future re-divergence on the same line.
  process.stderr.write(
    `contract-parity FAILED: ${stale.length} baseline entr${stale.length === 1 ? "y is" : "ies are"} stale ` +
      `(the divergence is gone — trim the baseline):\n` +
      stale.map((e) => `  - [${e.scope}] (${e.side}-only) ${e.line.slice(0, 140)}`).join("\n") +
      "\n\nRun `node scripts/cicd/check-contract-parity.mjs --update` to refresh.\n"
  );
  process.exit(1);
}

process.stdout.write(
  `contract-parity passed (platform tree: ${platformRoot}; ${baseline.entries.length} baselined deltas)\n`
);
