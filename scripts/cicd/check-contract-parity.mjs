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
 * Platform now consumes the public `@aexhq/contracts` package directly (via a
 * filesystem `link:` to this repo), so there is no longer a platform/contracts
 * mirror to compare. One axis remains:
 *   - the SSRF host deny-list, whose single source of truth is
 *     platform/shared/src/blueprint.ts. The public copy lives inline in
 *     run-config.ts (public has no blueprint.ts), so the three deny
 *     functions are compared directly so the Wave-1 hardening can't
 *     regress on one side only.
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
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
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
    // Require the tree the gate reads (the SSRF deny-list SoT), so a
    // misconfigured override skips (loud notice) rather than crashing mid-read.
    if (existsSync(join(c, "packages", "shared", "src", "blueprint.ts"))) {
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

function lineHash(line) {
  return createHash("sha256").update(line).digest("hex");
}

/** Stable fingerprint for a divergence, used as the baseline key. */
function fp(scope, side, hash) {
  return `${scope}|${side}|${hash}`;
}

function entryKey(entry) {
  return fp(entry.scope, entry.side, entry.lineHash ?? lineHash(entry.line ?? ""));
}

function foundEntry(scope, side, line) {
  return { scope, side, lineHash: lineHash(line) };
}

// Collect every current divergence across both axes.
const found = new Map(); // fp -> { scope, side, line }

// ---- SSRF deny-list parity (blueprint.ts <-> run-config.ts) ----------------
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
  const entry = foundEntry("deny-list", "n/a", "could not locate deny functions in one tree");
  found.set(entryKey(entry), entry);
} else {
  for (const d of diffLines(platformDeny, publicDeny)) {
    const entry = foundEntry("deny-list", d.side, d.line);
    found.set(entryKey(entry), entry);
  }
}

// ---- Baseline reconcile ----------------------------------------------------
let baseline = { entries: [] };
if (existsSync(baselinePath)) {
  baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
}
const baselineKeys = new Set(baseline.entries.map(entryKey));

if (UPDATE) {
  const merged = [...found.values()]
    .map((e) => {
      const existing = baseline.entries.find((b) => entryKey(b) === entryKey(e));
      return { ...e, why: existing?.why ?? "Explain this divergence before committing the baseline update." };
    })
    .sort((a, b) => entryKey(a).localeCompare(entryKey(b)));
  writeFileSync(baselinePath, JSON.stringify({ entries: merged }, null, 2) + "\n");
  process.stdout.write(`contract-parity: baseline updated (${merged.length} entries)\n`);
  process.exit(0);
}

const unexplained = [...found.values()].filter((e) => !baselineKeys.has(entryKey(e)));
const stale = baseline.entries.filter((e) => !found.has(entryKey(e)));

if (unexplained.length > 0) {
  process.stderr.write(
    `contract-parity FAILED: ${unexplained.length} NEW divergence${unexplained.length === 1 ? "" : "s"} not in the baseline:\n` +
      unexplained.map((e) => `  - [${e.scope}] (${e.side}-only) ${e.lineHash}`).join("\n") +
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
      stale.map((e) => `  - [${e.scope}] (${e.side}-only) ${e.lineHash ?? lineHash(e.line ?? "")}`).join("\n") +
      "\n\nRun `node scripts/cicd/check-contract-parity.mjs --update` to refresh.\n"
  );
  process.exit(1);
}

process.stdout.write(
  `contract-parity passed (platform tree: ${platformRoot}; ${baseline.entries.length} baselined deltas)\n`
);
