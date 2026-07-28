#!/usr/bin/env bun
/**
 * Cross-repo contract parity gate.
 *
 * This repo (public) is the single source of truth for the contract surface;
 * platform consumes `@aexhq/contracts` directly via a filesystem `link:` to
 * this repo, so there is no platform/contracts mirror to diff. The gate's sole
 * remaining axis is the SSRF host deny-list, which is duplicated by necessity:
 *   - the public copy lives inline in session-config.ts (public has no blueprint.ts);
 *   - the platform copy lives in platform/packages/shared/src/blueprint.ts.
 * The two are kept byte-identical so the Wave-1 SSRF hardening can't regress on
 * one side only — the three deny functions are compared directly here.
 *
 * A divergence fails the gate UNLESS it is recorded in the baseline
 * (`contract-parity-baseline.json`, same dir). The baseline is the explicit
 * allowlist: it pins KNOWN, intentional or tracked-temporary deny-list deltas
 * (e.g. public's de-branded doc comments inside the deny block, which strip
 * platform-internal infra references). Each baseline entry carries a `why`.
 * NEW divergence that isn't in the baseline fails — that's the tripwire.
 *
 * WHERE THIS RUNS. It is wired into `lint` (package.json), so it executes in
 * `.github/workflows/ci.yml` job `lint` and on every local pre-push
 * (`.githooks/pre-push` -> `scripts/pre-push.mjs`). The two behave differently
 * on purpose, and the difference is the whole design:
 *
 *   - PUBLIC CI can never see the private repo, so it SKIPS (exit 0) with a
 *     notice naming the exact path probed. That is a reported absence, not a
 *     silent pass.
 *   - A checkout that HAS the platform tree beside it — the canonical local
 *     workspace, and any private-side job that checks out both — ENFORCES.
 *
 * Because of that split, an explicitly configured `PLATFORM_DIR` that does not
 * resolve is a FAILURE, not a skip. Only an unconfigured, genuinely absent
 * sibling skips. A misconfigured enforcement path that looked like an absent
 * one would turn every consumer of this gate green for the wrong reason.
 *
 * Why the contract pipeline does not cover this: the Zod -> OpenAPI ->
 * generated-types chain describes WIRE SHAPES. The deny-list is imperative
 * validation logic that never reaches the document — grep
 * `packages/contracts/openapi/data-plane.json` for a deny reason and it is
 * absent. Platform's `scripts/validate/egress-cidr-ssot.test.ts` covers the
 * CIDR TABLE (reason vocabulary + specific ranges); it does not compare these
 * three classifier bodies. This gate is the only check that does.
 *
 * Run `bun scripts/cicd/check-contract-parity.mjs --update` after an
 * intentional, reviewed divergence to refresh the baseline.
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const publicRoot = resolve(here, "..", ".."); // aex/ (the public repo)
const baselinePath = join(here, "contract-parity-baseline.json");
const UPDATE = process.argv.includes("--update");

/** The tree this gate reads. Its presence is what "platform is available" means. */
const PLATFORM_SSOT = join("packages", "shared", "src", "blueprint.ts");
const configuredPlatformDir = process.env.PLATFORM_DIR?.trim();

function show(path) {
  return path.replaceAll("\\", "/");
}

// Explicit configuration is checked against exactly one candidate; absent
// configuration probes the sibling checkout used by the local workspace and by
// any job that checks out both repositories side by side.
const platformCandidates = configuredPlatformDir
  ? [resolve(configuredPlatformDir)]
  : [resolve(publicRoot, "..", "platform")];

const platformRoot =
  platformCandidates.find((candidate) => existsSync(join(candidate, PLATFORM_SSOT))) ?? null;

if (!platformRoot) {
  const probed = platformCandidates.map(show).join(", ");
  if (configuredPlatformDir) {
    // Fails CLOSED: someone asked for enforcement and did not get it. Skipping
    // here would report "no platform tree" for a tree that is meant to be there.
    process.stderr.write(
      `contract-parity FAILED: PLATFORM_DIR=${show(resolve(configuredPlatformDir))} holds no ` +
        `${show(PLATFORM_SSOT)} — the platform checkout is missing or misconfigured. ` +
        "Point PLATFORM_DIR at a platform checkout, or unset it to skip.\n"
    );
    process.exit(1);
  }
  process.stdout.write(
    `contract-parity: SKIPPED — no platform tree at ${probed} (${show(PLATFORM_SSOT)} absent). ` +
      "The public repo cannot check out the private one, so public CI and forks always take this " +
      "path; set PLATFORM_DIR=<platform checkout> to enforce.\n"
  );
  process.exit(0);
}

const publicContractsSrc = join(publicRoot, "packages", "contracts", "src");
const blueprintPath = join(platformRoot, "packages", "shared", "src", "blueprint.ts");
const publicSessionConfigPath = join(publicContractsSrc, "session-config.ts");

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

// Collect every current divergence on the deny-list axis.
const found = new Map(); // fp -> { scope, side, line }

// ---- SSRF deny-list parity (blueprint.ts <-> session-config.ts) ----------------
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
const publicDeny = extractDenyBlock(readNorm(publicSessionConfigPath));
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
      "`bun scripts/cicd/check-contract-parity.mjs --update` and fill in the " +
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
      "\n\nRun `bun scripts/cicd/check-contract-parity.mjs --update` to refresh.\n"
  );
  process.exit(1);
}

process.stdout.write(
  `contract-parity passed (platform tree: ${platformRoot}; ${baseline.entries.length} baselined deltas)\n`
);
