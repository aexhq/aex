/**
 * Single source of truth for the RELEASE-GATING live-test model.
 *
 * Gating suites (the platform deploy matrix) test PLATFORM behavior, not any
 * particular model vendor — so they all run on one cheap gateway model. Under
 * managed keys there is NO customer-supplied provider key: the platform's
 * single managed Vercel AI Gateway key routes all traffic, so a run needs only
 * a `creator/model` slug. Per-model correctness coverage lives in
 * test/live/providers/ and runs only via the non-gating, dispatch-only
 * `test:user:providers` suite.
 */

/** Canonical gate model slug (overridable; blank/whitespace treated as unset). */
export function gateModel(): string {
  return process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek/deepseek-v4-flash";
}
