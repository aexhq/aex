/**
 * C4 — switching the wire-conformance harness on for THIS suite.
 *
 * ## Why this is not three lines in `preload.ts`
 *
 * `installWireConformance()` attaches to a module-level observer inside
 * `@aexhq/contracts`, and it only ever sees responses that pass through the
 * `HttpClient` living in the SAME module graph. This package makes **no HTTP
 * requests in its own process**: every scenario writes a small script into a
 * fresh `bun install` tempdir and spawns `bun <script>` there, exactly as a
 * customer would run their own code. The SDK — and the copy of the contracts
 * package inlined into its `dist/_contracts/` — is loaded by the CHILD.
 *
 * So a harness installed in the test process would observe nothing, report zero
 * violations over zero responses, and read as a verified surface. That is the
 * precise failure `04-gates.md` says this gate exists to prevent, so the harness
 * is installed on BOTH sides:
 *
 * 1. **In every child**, through a `bunfig.toml` written into the install dir.
 *    Bun applies a cwd-local `bunfig.toml`'s top-level `preload` to `bun
 *    <script>`, and every child in this suite is spawned with `cwd = installDir`
 *    — so all of them are armed by one file, with no change at the ~40 spawn
 *    sites and no environment threading. The preload resolves the harness from
 *    the installed SDK's own `dist/_contracts/`, which is the only way to reach
 *    the observer instance the SDK's `HttpClient` reports to; importing
 *    `@aexhq/contracts` from this workspace would install an observer on a
 *    second, unrelated module object.
 * 2. **In the test process** ({@link installInProcessWireConformance}), so a
 *    future in-process request is covered rather than silently uncounted.
 *
 * Each process writes ONE JSON fragment; {@link readWireConformanceFragments}
 * folds them. The fold is where coverage becomes meaningful — no single process
 * sees more than a handful of routes.
 *
 * ## Plane collision
 *
 * `DATA_PLANE_RESPONSE_SCHEMAS` describes the data plane, and the control plane
 * serves different bodies at two of the same paths. The harness is therefore
 * always installed with `origin: AEX_API_URL`; anything from another origin is
 * counted as off-plane rather than judged. See `testing/response-bindings.ts`.
 */
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  DATA_PLANE_RESPONSE_SCHEMAS,
  installWireConformance,
  mergeWireConformanceReports,
  type WireConformanceReport
} from "@aexhq/contracts/testing";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..", "..");

/**
 * Where fragments are collected.
 *
 * The lane runner sets `AEX_WIRE_CONFORMANCE_DIR` and clears it first, so a
 * lane's aggregate covers that lane and nothing else. A bare `bun test` falls
 * back to the same default path — its per-process report is still exact; only a
 * cross-process aggregate could be polluted by an earlier run, and only the lane
 * runner computes one.
 */
export function wireConformanceRoot(): string {
  return process.env.AEX_WIRE_CONFORMANCE_DIR ?? join(appRoot, ".tmp", "wire-conformance");
}

/**
 * This test process's own fragment directory. One per process so that a file's
 * `afterAll` can fold exactly the children IT spawned, while the lane runner
 * folds every directory.
 */
let processDir: string | undefined;

function ownDirectory(): string {
  if (processDir === undefined) {
    processDir = join(wireConformanceRoot(), `proc-${process.pid}-${randomSuffix()}`);
    mkdirSync(processDir, { recursive: true });
  }
  return processDir;
}

function randomSuffix(): string {
  return Math.random().toString(36).slice(2, 10);
}

/**
 * The data plane these bindings describe.
 *
 * Read from the same env the suites read. **Absent, nothing is armed at all** —
 * and that is the point, not a shortcut. `DATA_PLANE_RESPONSE_SCHEMAS` describes
 * what a REAL server sends, while half this suite drives the SDK against a
 * scripted `fetch` at `https://example.invalid`. Judging a hand-written stub
 * against the wire contract would fail the offline lanes on bodies no server
 * ever sent — the opposite of observing reality, and it would push someone
 * toward weakening the schemas to make a fixture pass.
 *
 * With `AEX_API_URL` set, that same distinction is enforced per response rather
 * than per process: a stub's synthetic origin is reported off-plane and never
 * judged, while the real plane's bytes are.
 */
function dataPlaneOrigin(): string | undefined {
  const base = process.env.AEX_API_URL?.trim();
  return base ? base : undefined;
}

const NOT_ARMED_REASON =
  "AEX_API_URL is not set, so there is no plane whose bytes these bindings describe; " +
  "the suite's stub-fetch responses are deliberately NOT judged against the wire contract";

/** Test-only seams. Production callers pass none of these. */
export interface ArmWireConformanceOverrides {
  /** Where fragments land. Defaults to this process's own directory. */
  readonly outputDir?: string;
  /** The plane to judge against. Defaults to `AEX_API_URL`. */
  readonly origin?: string;
  /** The harness module to load. Defaults to the installed SDK's inlined copy. */
  readonly harnessPath?: string;
}

/**
 * Write the `bunfig.toml` + preload that arm every `bun` child run in
 * `installDir`. Returns whether anything was armed.
 *
 * Called by the install fixture, so a scenario gets C4 by installing the SDK —
 * there is nothing for a test author to remember. Every path baked into the
 * generated preload is absolute: the child's cwd is `installDir`, but the
 * fragment directory is deliberately OUTSIDE it, because each test file removes
 * its install tree in `afterAll` and would take the evidence with it.
 */
export function armWireConformance(
  installDir: string,
  overrides: ArmWireConformanceOverrides = {}
): boolean {
  const origin = overrides.origin ?? dataPlaneOrigin();
  if (origin === undefined) return false;

  const outputDir = overrides.outputDir ?? ownDirectory();
  const harness =
    overrides.harnessPath ??
    join(
      installDir,
      "node_modules",
      "@aexhq",
      "sdk",
      "dist",
      "_contracts",
      "testing",
      "wire-conformance-entry.js"
    );

  writeFileSync(join(installDir, "aex-wire-conformance.mjs"), childPreloadSource(harness, outputDir, origin));
  // Bun reads `bunfig.toml` from the process cwd, and every child in this suite
  // runs with cwd = installDir. A top-level `preload` (NOT `[test] preload`)
  // applies to `bun <script>`, which is how these children are spawned.
  writeFileSync(join(installDir, "bunfig.toml"), 'preload = ["./aex-wire-conformance.mjs"]\n');
  return true;
}

/**
 * The child-side preload, as source.
 *
 * Written rather than imported because it must run inside the install tree with
 * the installed SDK's own module graph, and because it must be able to fail
 * SOFT: a child that cannot arm the harness still has a scenario to run, and
 * turning every spawn into a hard failure would make this switch-on a
 * suite-wide outage rather than a gate. The failure is recorded in the fragment
 * as an `armFailure`, so a run where nothing armed reports that instead of
 * reporting a clean sweep.
 */
function childPreloadSource(harnessPath: string, outputDir: string, origin: string): string {
  return `// GENERATED by apps/user-tests/test/_fixtures/wire-conformance.ts — do not edit.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const OUTPUT_DIR = ${JSON.stringify(outputDir)};
const HARNESS = ${JSON.stringify(harnessPath)};
const ORIGIN = ${JSON.stringify(origin)};

function emit(fragment) {
  try {
    mkdirSync(OUTPUT_DIR, { recursive: true });
    writeFileSync(
      join(OUTPUT_DIR, "child-" + process.pid + "-" + Math.random().toString(36).slice(2, 10) + ".json"),
      JSON.stringify(fragment)
    );
  } catch (error) {
    process.stderr.write("[c4] could not write wire-conformance fragment: " + String(error) + "\\n");
  }
}

try {
  const { DATA_PLANE_RESPONSE_SCHEMAS, installWireConformance } = await import(HARNESS);
  const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: ORIGIN });
  process.on("exit", () => {
    emit(harness.report());
  });
} catch (error) {
  // The installed SDK predates the harness, or the tarball omitted it. Say so
  // in the fragment rather than letting the run look clean.
  emit({ armFailure: HARNESS + ": " + String(error) });
}
`;
}

/**
 * Install the harness in the TEST process.
 *
 * Returns a `finish()` that persists this process's own fragment. Zero
 * observation is the expected result today — the suite drives the SDK out of
 * process — and the fragment is written anyway so a regression that moves
 * requests in-process is covered from the first one.
 *
 * With no data plane configured it installs nothing and records WHY, so a lane
 * that checked nothing reports that rather than reporting a clean sweep.
 */
export function installInProcessWireConformance(): { readonly finish: () => void } {
  const origin = dataPlaneOrigin();
  if (origin === undefined) {
    return {
      finish: () => {
        write(`skip-${process.pid}.json`, { notArmed: NOT_ARMED_REASON });
      }
    };
  }
  const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin });
  return {
    finish: () => {
      const report = harness.report();
      harness.stop();
      write(`self-${process.pid}.json`, report);
    }
  };
}

function write(name: string, fragment: Fragment): void {
  writeFileSync(join(ownDirectory(), name), JSON.stringify(fragment));
}

/** A fragment: a report, a process that could not arm, or one that chose not to. */
type Fragment =
  | WireConformanceReport
  | { readonly armFailure: string }
  | { readonly notArmed: string };

export interface WireConformanceHarvest {
  readonly report: WireConformanceReport;
  /** Processes that TRIED to attach the harness and could not, with the reason. */
  readonly armFailures: readonly string[];
  /** Processes that deliberately did not attach it, with the reason. */
  readonly notArmed: readonly string[];
  /** Fragments folded in — 0 means nothing ran, NOT that everything passed. */
  readonly fragments: number;
  /** Fragments that carried an actual report. */
  readonly reports: number;
}

/**
 * Fold every fragment under `root` (recursively).
 *
 * The two non-report outcomes are surfaced separately and NEVER merged into the
 * report. A run whose children all failed to arm would otherwise produce a
 * perfectly clean "0 violations" over zero observations, which is the false
 * green this whole gate exists to prevent.
 */
export function readWireConformanceFragments(root = wireConformanceRoot()): WireConformanceHarvest {
  const reports: WireConformanceReport[] = [];
  const armFailures: string[] = [];
  const notArmed = new Set<string>();
  let fragments = 0;

  for (const path of jsonFilesUnder(root)) {
    let parsed: Fragment;
    try {
      parsed = JSON.parse(readFileSync(path, "utf8")) as Fragment;
    } catch (error) {
      armFailures.push(`${path}: unreadable fragment (${String(error)})`);
      continue;
    }
    fragments += 1;
    if ("armFailure" in parsed) armFailures.push(parsed.armFailure);
    else if ("notArmed" in parsed) notArmed.add(parsed.notArmed);
    else reports.push(parsed);
  }

  return {
    report: mergeWireConformanceReports(reports),
    armFailures,
    notArmed: [...notArmed],
    fragments,
    reports: reports.length
  };
}

/** Fold only the fragments this process and its children produced. */
export function readOwnWireConformanceFragments(): WireConformanceHarvest {
  return readWireConformanceFragments(ownDirectory());
}

function jsonFilesUnder(root: string): string[] {
  let entries;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    return [];
  }
  const found: string[] = [];
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) found.push(...jsonFilesUnder(path));
    else if (entry.name.endsWith(".json")) found.push(path);
  }
  return found;
}
