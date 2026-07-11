/**
 * Single source of truth for the RELEASE-GATING live-test provider.
 *
 * Gating suites (release.yml smoke plus the discovered live-test matrix) test
 * PLATFORM behavior, not any particular model vendor — so they all run on one
 * cheap, funded provider and must never depend on another provider account's
 * billing state (2026-07-03: the shared BYOK ANTHROPIC_API_KEY ran out of
 * credit and killed multiple gating jobs while DeepSeek + managed-key tests
 * passed). Per-provider correctness coverage (Anthropic BYOK, doubao, …)
 * lives in test/live/providers/ and runs only via the non-gating, dispatch-only
 * `test:user:providers` suite (live-on-demand-tests.yml).
 */

/** The provider every gating live test submits with. */
export const GATE_PROVIDER = "deepseek" as const;

/** Env var carrying the gate provider's BYOK key. */
export const GATE_KEY_ENV = "DEEPSEEK_API_KEY" as const;

/** Canonical gate model id (overridable; blank/whitespace treated as unset). */
export function gateModel(): string {
  return process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";
}

/** The gate provider's BYOK key, or a loud failure naming the suite. */
export function requireGateKey(context: string): string {
  const value = process.env[GATE_KEY_ENV];
  if (!value || value.length === 0) {
    throw new Error(
      `${context}: required env ${GATE_KEY_ENV} is missing. Release-gating live tests run on the ${GATE_PROVIDER} gate provider only.`
    );
  }
  return value;
}

/** `apiKeys` map for a gating submission, built from ${GATE_KEY_ENV}. */
export function gateApiKeys(context: string): Record<string, string> {
  return { [GATE_PROVIDER]: requireGateKey(context) };
}
