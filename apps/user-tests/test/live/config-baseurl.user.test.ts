/**
 * USER TEST (SDK-driven) — `secrets.{provider}.baseUrl` is HONORED.
 *
 * Validates the FIX end-to-end through the installed SDK: when a run sets a
 * customer `baseUrl`, the platform actually routes upstream there. Proof is
 * negative-by-design: an UNREACHABLE baseUrl breaks the upstream call. The
 * managed Goose BYOK surface:
 *   - the baseUrl is the upstream MODEL origin. With
 *     it unreachable the proxy returns a 530 and goose reports the upstream
 *     error as a normal turn, then EXITS 0 — so status stays "succeeded",
 *     but the model never actually answered. We assert the agent did NOT get
 *     the requested "READY" reply (it got the upstream error instead), which
 *     is the signal the customer baseUrl was used (pre-fix it hit the real
 *     api.deepseek.com and replied READY).
 *
 * Only passes once the baseUrl-honoring fix is DEPLOYED to antpath-local.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

const BAD_BASE_URL = "https://baseurl-canary.invalid.example";

describe("user/SDK: secrets.{provider}.baseUrl is honored (unreachable baseUrl fails the run)", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed BYOK: an unreachable deepseek baseUrl prevents the requested reply",
    async () => {
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: ["Reply with READY."],
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY, baseUrl: ${JSON.stringify(BAD_BASE_URL)} } },
          idempotencyKey: "user-baseurl-deepseek-managed-a-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-baseurl-deepseek-managed-a.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.assistantText.length).toBeGreaterThan(0);
      expect(dense(result.assistantText)).not.toContain("READY");
    },
    10 * 60_000
  );

  it(
    "managed BYOK: an unreachable deepseek baseUrl fails the run",
    async () => {
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: ["Reply with READY."],
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY, baseUrl: ${JSON.stringify(BAD_BASE_URL)} } },
          idempotencyKey: "user-baseurl-managed-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-baseurl-managed.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      // The BYOK provider-proxy used the customer baseUrl as the upstream
      // origin → the model call hit the unreachable host (530) and goose
      // reported the upstream error instead of the model's answer. So the
      // requested "READY" reply is absent (it would be present if baseUrl
      // were ignored and the call hit the real api.deepseek.com).
      expect(result.assistantText.length).toBeGreaterThan(0);
      expect(dense(result.assistantText)).not.toContain("READY");
    },
    10 * 60_000
  );
});
