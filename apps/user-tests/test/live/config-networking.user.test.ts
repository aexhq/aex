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
      const probe = `set -u
probe_https() {
  reached="$1"
  blocked="$2"
  url="$3"
  name="$(printf "%s" "$reached" | tr "A-Z" "a-z")"
  body="/tmp/aex-networking-$name.body"
  err="/tmp/aex-networking-$name.err"
  code="$(curl -sS -m 10 -o "$body" -w "%{http_code}" "$url" 2>"$err" || true)"
  if [ -n "$code" ] && [ "$code" != "000" ]; then
    printf "%s_HTTP_%s\\n" "$reached" "$code"
  else
    printf "%s_HTTP_%s\\n" "$blocked" "\${code:-000}"
  fi
}
a="$(probe_https ALLOWED_REACHED ALLOWED_FAILED https://example.com/)"
o="$(probe_https OTHER_REACHED OTHER_BLOCKED https://api.github.com/)"
printf "%s %s\\n" "$a" "$o"`;
      const prompt =
        "Using the shell, run exactly this bash script without replacing it with curl exit-code shortcuts. " +
        "Classify reachability from HTTP status: HTTP 000, connection refused, timeout, or proxy/gate failure means BLOCKED. " +
        `Script:\n${probe}\n` +
        "Reply with ONLY the final two probe tokens separated by a single space.";
      const script = sdkRunnerScript({
        run: `{
          provider: "deepseek",
          model: MODEL_DEEPSEEK,
          message: ${JSON.stringify(prompt)},
          environment: { networking: { mode: "limited", allowedHosts: ["example.com"] } },
          apiKeys: { deepseek: DEEPSEEK_KEY },
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

      // Drive the assertion off the REAL curl evidence (tool_result) joined
      // with the model's narration, not just the LLM's self-reported token.
      const shellEvidence = dense(`${result.toolResultText} ${result.assistantText}`);
      // Precision: the customer's allowed host worked...
      expect(shellEvidence).toContain("ALLOWED_REACHED");
      // ...and the non-allowed host was blocked.
      expect(shellEvidence).toContain("OTHER_BLOCKED");
      // Negative guards: a wide-open firewall would surface the failure
      // tokens for the allowed host or the reached token for the other host.
      expect(shellEvidence).not.toContain("ALLOWED_FAILED");
      expect(shellEvidence).not.toContain("OTHER_REACHED");
    },
    10 * 60_000
  );
});
