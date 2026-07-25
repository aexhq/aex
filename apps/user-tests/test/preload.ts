/**
 * bun:test preload — loads `.env.local` (live target vars, provider keys)
 * before any test file is collected, replacing the `loadLocalEnv()` call the
 * retired vitest lane configs made at config-eval time. Runs once per test
 * process (`--isolate` gives one process per file).
 *
 * No mock-restore hook here on purpose: this package spawns child processes
 * instead of mocking (zero spies/mocks in the suites).
 *
 * It also switches on C4, the wire-conformance gate: the harness is installed
 * before any test file is imported, and a root `afterAll` folds what this
 * process and its children observed, prints it, and FAILS the file on any
 * violation. `_fixtures/wire-conformance.ts` explains why the interesting half
 * of that happens in the children rather than here.
 */
import { afterAll } from "bun:test";
import { loadLocalEnv } from "./env-local";
import {
  installInProcessWireConformance,
  readOwnWireConformanceFragments
} from "./_fixtures/wire-conformance";
import { formatWireConformanceReport } from "@aexhq/contracts/testing";

loadLocalEnv();

const inProcess = installInProcessWireConformance();

afterAll(() => {
  inProcess.finish();
  const { report, armFailures, reports } = readOwnWireConformanceFragments();

  // Only speak up when this file actually observed something. Coverage is a
  // property of the RUN, not of one file — no single file exercises more than a
  // handful of routes — so the full breakdown is printed once, by the lane
  // runner in `scripts/user-bun-test.mjs`. Failing here as well is what puts the
  // violation in the junit report next to the test that produced it.
  if (report.observed > 0) {
    console.error(
      `[c4] ${reports} process(es): ${report.observed} response(s), ` +
        `${report.validated.length} operation(s) validated, ` +
        `${report.errorsValidated.length} error response(s) checked, ` +
        `${report.offPlane.length} off-plane, ${report.violations.length} violation(s)`
    );
  }

  if (armFailures.length > 0) {
    // Never silent: a child that could not attach the harness contributed no
    // observations, and a run of those would otherwise read as a clean sweep.
    throw new Error(
      `[c4] ${armFailures.length} process(es) could not attach the wire-conformance harness:\n` +
        armFailures.map((failure) => `  - ${failure}`).join("\n")
    );
  }
  if (report.violations.length > 0) {
    throw new Error(`[c4] wire conformance violated\n${formatWireConformanceReport(report)}`);
  }
});
