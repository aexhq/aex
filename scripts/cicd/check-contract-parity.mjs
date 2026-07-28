#!/usr/bin/env bun
/**
 * Cross-repo contract parity gate.
 *
 * This repo (public) is the single source of truth for the contract surface;
 * platform consumes `@aexhq/contracts` directly, so there is no platform
 * contracts mirror to diff. The gate's sole axis is the SSRF host deny-list,
 * which is duplicated by necessity:
 *   - the public copy lives inline in session-config.ts (public has no blueprint.ts);
 *   - the platform copy lives in platform/packages/shared/src/blueprint.ts.
 * The two are kept in parity so the Wave-1 SSRF hardening cannot regress on one
 * side only — the three deny functions are compared directly here.
 *
 * Why the contract pipeline does not cover this: the Zod -> OpenAPI ->
 * generated-types chain describes WIRE SHAPES. The deny-list is imperative
 * validation logic that never reaches the document — grep
 * `packages/contracts/openapi/data-plane.json` for a deny reason and it is
 * absent. Platform's `scripts/validate/egress-cidr-ssot.test.ts` covers the
 * CIDR TABLE (reason vocabulary + specific ranges); it does not compare these
 * classifier bodies. This gate is the only check that does.
 *
 * ---------------------------------------------------------------------------
 * TWO DIFFS, ONE GATE
 * ---------------------------------------------------------------------------
 * The check used to gate on EXACT line equality of the whole extracted region,
 * comments included. That is what manufactured a baseline file: all nine
 * entries it carried were doc-comment wording — public strips platform-internal
 * infra references, platform names its own backlog — and not one of them was a
 * behavioural difference. A ratchet whose entries are all prose is a ratchet
 * that will be updated without being read.
 *
 * So the check is split, the way a schema diff separates "what changed" from
 * "what breaks":
 *
 *   INFORMATION — the full line diff, comments and all. Always computed, always
 *                 printed when non-empty. Never fails.
 *   GATE        — the same diff over EXECUTABLE TEXT only: comments stripped,
 *                 blank lines dropped, lines trimmed. A divergence here is a
 *                 divergence in what the two trees actually DO.
 *
 * A divergence in the gate half fails UNLESS it is recorded in the baseline
 * (`contract-parity-baseline.json`, same dir). The baseline is a DEBT REGISTER,
 * not an allowlist: every entry carries a `why`, a named `owner`, and a hard
 * `expiresAt` at most 30 days out — the same contract platform's
 * `scripts/cicd/quarantine.mjs` puts on a quarantined test, for the same reason.
 * An expired entry FAILS; that failure is the mechanism working. And the entry
 * COUNT cannot grow: `maxEntries` is a ratchet that `--update` may lower and
 * never raise, so admitting a genuinely new divergence is a reviewed hand edit
 * rather than a tool invocation.
 *
 * ---------------------------------------------------------------------------
 * WHERE THIS RUNS, AND WHAT PINS IT
 * ---------------------------------------------------------------------------
 * Wired into `lint` (package.json), so it executes in `.github/workflows/ci.yml`
 * job `lint` and on every local pre-push. Three states, and the difference
 * between them is the whole design:
 *
 *   1. AEX_REQUIRE_PARITY=1 — ENFORCE OR FAIL. A missing platform tree is a
 *      misconfiguration, not a valid state, so the skip path exits 1. This is
 *      what the private-side verification job sets; see
 *      `.github/workflows/contract-parity.yml` for how a public PR reaches it.
 *   2. a platform tree is present — ENFORCE, and report the exact platform
 *      commit compared.
 *   3. neither — SKIP (exit 0) with a notice naming the path probed. Public CI
 *      and forks cannot check out the private repo, so they always take this
 *      path; the dispatch workflow is what makes that skip harmless rather than
 *      a hole. A reported absence, not a silent pass.
 *
 * An explicitly configured `PLATFORM_DIR` that does not resolve is a FAILURE in
 * every state: a misconfigured enforcement path that looked like an absent one
 * would turn every consumer of this gate green for the wrong reason.
 *
 * PLATFORM_REF pins WHICH platform object is compared. Under
 * AEX_REQUIRE_PARITY it is REQUIRED and must be a 40-character commit SHA —
 * the same immutability rule this repo applies to GitHub Action pins, and for
 * the same reason. Comparing against a branch tip proves compatibility with
 * whatever happened to be at the tip during the run, which is not the object
 * any release consumes; the release consumes an exact commit, so the gate names
 * an exact commit. Outside enforcement mode the pin is optional and the sibling
 * tree's HEAD is reported instead of assumed.
 *
 * Run `bun scripts/cicd/check-contract-parity.mjs --update` after an
 * intentional, reviewed divergence to refresh the baseline.
 */
import { execFileSync } from "node:child_process";
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
const COMMIT_SHA = /^[0-9a-f]{40}$/;
const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;
const MAX_BASELINE_DAYS = 30;
const DAY_MS = 86_400_000;

const configuredPlatformDir = process.env.PLATFORM_DIR?.trim();
const configuredPlatformRef = process.env.PLATFORM_REF?.trim();
const requireParity = process.env.AEX_REQUIRE_PARITY?.trim() === "1";
const now = Date.now();

function show(path) {
  return path.replaceAll("\\", "/");
}

function fail(message) {
  process.stderr.write(`contract-parity FAILED: ${message}\n`);
  process.exit(1);
}

// ---- Locate and pin the platform tree ---------------------------------------

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
    fail(
      `PLATFORM_DIR=${show(resolve(configuredPlatformDir))} holds no ${show(PLATFORM_SSOT)} — ` +
        "the platform checkout is missing or misconfigured. Point PLATFORM_DIR at a platform " +
        "checkout, or unset it to skip."
    );
  }
  if (requireParity) {
    // The whole point of AEX_REQUIRE_PARITY. Without it this branch exits 0, and
    // an exit 0 from a job that was supposed to be THE gate is indistinguishable
    // from the gate passing.
    fail(
      `AEX_REQUIRE_PARITY=1 but no platform tree was found at ${probed} ` +
        `(${show(PLATFORM_SSOT)} absent). In enforcement mode a missing platform checkout is a ` +
        "misconfiguration, not a valid state. Set PLATFORM_DIR at a platform checkout, or unset " +
        "AEX_REQUIRE_PARITY if this job is not the gate."
    );
  }
  process.stdout.write(
    `contract-parity: SKIPPED — no platform tree at ${probed} (${show(PLATFORM_SSOT)} absent). ` +
      "The public repo cannot check out the private one, so public CI and forks always take this " +
      "path; .github/workflows/contract-parity.yml dispatches the private-side run instead. " +
      "Set PLATFORM_DIR=<platform checkout> to enforce here, and AEX_REQUIRE_PARITY=1 to make " +
      "this skip a failure.\n"
  );
  process.exit(0);
}

/**
 * The exact platform commit compared, or null when the tree is not a git
 * checkout. Memoised, and skipped entirely when there is no `.git` — the gate
 * runs inside `lint` on every pre-push and a subprocess per invocation is a
 * cost it does not need to pay to report a path it already knows.
 */
let headCache;
function platformHead() {
  if (headCache !== undefined) return headCache;
  if (!existsSync(join(platformRoot, ".git"))) {
    headCache = null;
    return headCache;
  }
  try {
    headCache = execFileSync("git", ["-C", platformRoot, "rev-parse", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"]
    }).trim();
  } catch {
    headCache = null;
  }
  return headCache;
}

if (requireParity && !configuredPlatformRef) {
  fail(
    "AEX_REQUIRE_PARITY=1 requires PLATFORM_REF, the 40-character platform commit this run " +
      "compares against. Without it the gate proves compatibility with whatever happened to be " +
      "checked out, which is not the object any release consumes — the release consumes an exact " +
      `commit. This tree is at ${platformHead() ?? "(not a git checkout)"}.`
  );
}

if (configuredPlatformRef) {
  if (!COMMIT_SHA.test(configuredPlatformRef)) {
    fail(
      `PLATFORM_REF=${configuredPlatformRef} is not a 40-character commit SHA. A branch or tag ` +
        "name is a mutable pointer, so pinning to one records nothing reproducible."
    );
  }
  const pinned = platformHead();
  if (pinned === null) {
    fail(
      `PLATFORM_REF=${configuredPlatformRef} was given but ${show(platformRoot)} is not a git ` +
        "checkout, so the pin cannot be verified."
    );
  }
  if (pinned !== configuredPlatformRef) {
    fail(
      `PLATFORM_REF=${configuredPlatformRef} but ${show(platformRoot)} is at ${pinned}. ` +
        "Check the platform tree out at the pinned commit; comparing a different object would " +
        "report parity with a version that is not the one under test."
    );
  }
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

/**
 * Split source into executable text and comment text.
 *
 * A small state machine rather than a regex, because a regex that strips `//`
 * to end of line also strips the inside of `"https://…"`. Quote and template
 * states are tracked; a `/` is a comment opener only when the next character is
 * `/` or `*`, which is exactly right for this region — no regex literal in it
 * contains either sequence, and `denyBlockIsIntact` below refuses to gate on a
 * stripped body that lost its function signatures.
 */
export function splitComments(text) {
  let code = "";
  let comments = "";
  let state = "code";
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    const next = text[i + 1];
    if (state === "code") {
      if (ch === "/" && next === "/") {
        state = "line";
        i += 1;
        continue;
      }
      if (ch === "/" && next === "*") {
        state = "block";
        i += 1;
        continue;
      }
      if (ch === '"' || ch === "'" || ch === "`") state = ch;
      code += ch;
      continue;
    }
    if (state === "line") {
      if (ch === "\n") {
        state = "code";
        comments += "\n";
        code += "\n";
        continue;
      }
      comments += ch;
      continue;
    }
    if (state === "block") {
      if (ch === "*" && next === "/") {
        state = "code";
        i += 1;
        comments += "\n";
        continue;
      }
      comments += ch;
      continue;
    }
    // inside a string or template literal
    code += ch;
    if (ch === "\\") {
      code += next ?? "";
      i += 1;
      continue;
    }
    if (ch === state) state = "code";
  }
  return { code, comments };
}

/** Executable text, normalised for comparison: trimmed lines, no blanks. */
function executableText(text) {
  return splitComments(text)
    .code.split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .join("\n");
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

// ---- Extract the axis --------------------------------------------------------

function extractDenyBlock(text) {
  const start = text.indexOf("denyReasonForHostIp");
  const end = text.indexOf("parseRemoteMcpTransport", start);
  if (start === -1 || end === -1) return null;
  const docStart = text.lastIndexOf("/**", start);
  return text
    .slice(docStart === -1 ? start : docStart, end)
    .replace(/\bexport function\b/g, "function") // public keeps these local (intentional surface delta)
    .trim();
}

/**
 * Non-vacuity: every `function <name>(` present in the raw block must survive
 * comment stripping. Derived from the block rather than hard-coded, so a fourth
 * classifier is covered the day it is written. A stripped body that lost its
 * signatures would diff clean against anything and pass for the wrong reason.
 */
export function missingSignatures(raw, code) {
  const names = [...raw.matchAll(/\bfunction\s+([A-Za-z_$][\w$]*)\s*\(/g)].map((m) => m[1]);
  return [...new Set(names)].filter((name) => !new RegExp(`\\bfunction\\s+${name}\\s*\\(`).test(code));
}

const platformDeny = extractDenyBlock(readNorm(blueprintPath));
const publicDeny = extractDenyBlock(readNorm(publicSessionConfigPath));

const found = new Map(); // fp -> { scope, side, lineHash }
let informational = [];

if (platformDeny === null || publicDeny === null) {
  const entry = foundEntry("deny-list", "n/a", "could not locate deny functions in one tree");
  found.set(entryKey(entry), entry);
} else {
  informational = diffLines(platformDeny, publicDeny);

  const platformCode = executableText(platformDeny);
  const publicCode = executableText(publicDeny);
  for (const [label, raw, code] of [
    ["platform", platformDeny, platformCode],
    ["public", publicDeny, publicCode]
  ]) {
    const lost = missingSignatures(raw, code);
    if (lost.length > 0) {
      fail(
        `the ${label} deny block lost ${lost.join(", ")} after comment stripping — the gate would ` +
          "compare a region that is missing the code it exists to compare, and pass vacuously. " +
          "Fix the extraction before trusting it."
      );
    }
  }

  for (const d of diffLines(platformCode, publicCode)) {
    const entry = foundEntry("deny-list", d.side, d.line);
    found.set(entryKey(entry), entry);
  }
}

// ---- Baseline: a debt register with owners, expiries, and a ratchet ----------

let baseline = { maxEntries: 0, entries: [] };
if (existsSync(baselinePath)) {
  baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
}
if (!Array.isArray(baseline.entries)) fail(`${show(baselinePath)} must have an "entries" array`);
if (!Number.isInteger(baseline.maxEntries) || baseline.maxEntries < 0) {
  fail(`${show(baselinePath)} must declare an integer "maxEntries" ratchet (0 means "none allowed")`);
}
if (baseline.entries.length > baseline.maxEntries) {
  fail(
    `${show(baselinePath)} carries ${baseline.entries.length} entries but its maxEntries ratchet is ` +
      `${baseline.maxEntries}. The ratchet may only be lowered.`
  );
}

/** Owner + expiry, on the model of platform's quarantine registry. */
function baselineViolations(entries) {
  const violations = [];
  entries.forEach((entry, index) => {
    const at = `entries[${index}]`;
    for (const field of ["scope", "side", "lineHash", "why", "owner", "recordedAt", "expiresAt"]) {
      if (typeof entry[field] !== "string" || entry[field].trim() === "") {
        violations.push(`${at}.${field} is missing — an unowned, unexpiring baseline entry is permanent debt`);
      }
    }
    for (const field of ["recordedAt", "expiresAt"]) {
      if (typeof entry[field] === "string" && !DATE_ONLY.test(entry[field])) {
        violations.push(`${at}.${field} ${JSON.stringify(entry[field])} must be YYYY-MM-DD`);
      }
    }
    if (!DATE_ONLY.test(String(entry.recordedAt)) || !DATE_ONLY.test(String(entry.expiresAt))) return;
    const recorded = Date.parse(`${entry.recordedAt}T00:00:00Z`);
    const expires = Date.parse(`${entry.expiresAt}T00:00:00Z`);
    if (Number.isNaN(recorded) || Number.isNaN(expires)) {
      violations.push(`${at} has an unparseable date`);
      return;
    }
    if (expires <= recorded) {
      violations.push(`${at}.expiresAt (${entry.expiresAt}) must be after recordedAt (${entry.recordedAt})`);
    }
    if ((expires - recorded) / DAY_MS > MAX_BASELINE_DAYS) {
      violations.push(
        `${at} spans ${((expires - recorded) / DAY_MS).toFixed(0)} days; the ceiling is ${MAX_BASELINE_DAYS}`
      );
    }
    if (expires <= now) {
      violations.push(
        `${at} EXPIRED on ${entry.expiresAt} and is owned by ${entry.owner} — port the change, or ` +
          "renew the entry deliberately in a reviewed PR"
      );
    }
  });
  return violations;
}

if (UPDATE) {
  const merged = [...found.values()]
    .map((e) => {
      const existing = baseline.entries.find((b) => entryKey(b) === entryKey(e));
      return {
        ...e,
        why: existing?.why ?? "Explain this divergence before committing the baseline update.",
        owner: existing?.owner ?? "",
        recordedAt: existing?.recordedAt ?? "",
        expiresAt: existing?.expiresAt ?? ""
      };
    })
    .sort((a, b) => entryKey(a).localeCompare(entryKey(b)));

  if (merged.length > baseline.maxEntries) {
    // The ratchet. `--update` shrinks the allowance and never widens it, so
    // admitting a NEW divergence is a hand edit somebody reviews, exactly as
    // adding a quarantine entry is.
    fail(
      `--update would record ${merged.length} entr${merged.length === 1 ? "y" : "ies"}, above the ` +
        `maxEntries ratchet of ${baseline.maxEntries}. Port the divergence into public, or raise ` +
        "maxEntries by hand in a reviewed PR and fill in why/owner/expiresAt for each new entry."
    );
  }
  writeFileSync(baselinePath, `${JSON.stringify({ maxEntries: merged.length, entries: merged }, null, 2)}\n`);
  process.stdout.write(
    `contract-parity: baseline updated (${merged.length} entries, ratchet lowered to ${merged.length})\n`
  );
  process.exit(0);
}

const violations = baselineViolations(baseline.entries);
if (violations.length > 0) {
  process.stderr.write(
    `contract-parity FAILED: the baseline has ${violations.length} policy violation(s):\n` +
      violations.map((v) => `  - ${v}`).join("\n") +
      "\n\nA baselined divergence is debt with a due date, not an amnesty.\n"
  );
  process.exit(1);
}

const baselineKeys = new Set(baseline.entries.map(entryKey));
const unexplained = [...found.values()].filter((e) => !baselineKeys.has(entryKey(e)));
const stale = baseline.entries.filter((e) => !found.has(entryKey(e)));

// The informational half. Printed BEFORE any verdict so a failure is read with
// the full picture, and printed on success too so drift that is invisible to the
// gate is still visible to a person.
if (informational.length > 0) {
  process.stdout.write(
    `contract-parity: ${informational.length} line-level difference(s) in the deny block, ` +
      "comments included. These are INFORMATION, not the gate — the gate compares executable " +
      "text only.\n" +
      informational.map((d) => `  · (${d.side}-only) ${d.line}`).join("\n") +
      "\n"
  );
}

if (unexplained.length > 0) {
  process.stderr.write(
    `contract-parity FAILED: ${unexplained.length} NEW behavioural divergence${unexplained.length === 1 ? "" : "s"} not in the baseline:\n` +
      unexplained.map((e) => `  - [${e.scope}] (${e.side}-only) ${e.lineHash}`).join("\n") +
      "\n\nThis is a difference in what the two trees DO, not in how they are " +
      "commented. Port the platform change into public, or — if the divergence " +
      "is intentional public-only surface — raise the maxEntries ratchet in a " +
      "reviewed PR and run `bun scripts/cicd/check-contract-parity.mjs --update`, " +
      "filling in why/owner/expiresAt for each new baseline entry.\n"
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
  `contract-parity passed (platform ${platformHead() ?? "tree"} at ${show(platformRoot)}; ` +
    `${baseline.entries.length} baselined delta(s), ratchet ${baseline.maxEntries})\n`
);
