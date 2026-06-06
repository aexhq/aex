/**
 * USER TEST (SDK-driven) — environment.networking allowlist works as expected.
 *
 * Validates the FIX end-to-end through the installed SDK: on a managed
 * (managed runtime) run with `networking.mode:"limited"`, an explicitly ALLOWED host
 * stays reachable while a non-allowed host is BLOCKED — proving the OS-level
 * egress firewall is precise, not a
 * block-everything sledgehammer.
 *
 * Allowed `example.com` vs blocked `api.github.com` are chosen because they
 * resolve to DIFFERENT IPs (the firewall allowlists by resolved IP), so a
 * pass genuinely proves per-host precision.
 *
 * Only passes once the worker is deployed AND the runner image is rebuilt
 * (the firewall lives in the image) — see the api/test/live networking probe.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: managed networking:limited allowlist is precise (allowed reachable, others blocked)", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "an allowed host is reachable and a non-allowed host is blocked on the same managed run",
    async () => {
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: [
            "Using the shell, make two separate HTTPS GET requests with curl -sS -m 10:",
            "Judge each by whether you actually REACHED the site (got its real response), not merely whether any bytes came back — a proxy 403/Forbidden means BLOCKED, not reached.",
            "1) https://example.com — if you reached the host and got its normal response, note the token ALLOWED_REACHED; if blocked/refused/timed out/proxy-403, note ALLOWED_FAILED.",
            "2) https://api.github.com — if you reached the host and got its normal API response, note the token OTHER_REACHED; if blocked/refused/timed out/proxy-403, note OTHER_BLOCKED.",
            "Reply with ONLY the two tokens separated by a space."
          ],
          environment: { networking: { mode: "limited", allowedHosts: ["example.com"] } },
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-networking-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-networking.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      // The run still SUCCEEDS — the platform proxy/model host is always-allowed
      // through the firewall, so the agent could run at all.
      expect(result.status).toBe("succeeded");

      const text = dense(result.assistantText);
      // Precision: the customer's allowed host worked...
      expect(text).toContain("ALLOWED_REACHED");
      // ...and the non-allowed host was blocked.
      expect(text).toContain("OTHER_BLOCKED");
    },
    10 * 60_000
  );
});
