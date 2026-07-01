/**
 * USER TEST (SDK-driven) — environment.packages pre-installs on managed runs.
 *
 * Validates the FIX end-to-end through the installed SDK: a customer-supplied
 * `environment.packages` entries are PRE-INSTALLED before the agent runs, so
 * the agent finds them WITHOUT installing anything itself. The managed runner
 * validates BOTH apt and pip — an unprefixed "jq" (→ apt) AND a "pip:cowsay"
 * (→ pip).
 *
 * Only passes once the fixes are DEPLOYED to the remote aex-local worker
 * (the managed-runtime half ALSO requires the runner image to be rebuilt, since package
 * pre-install now happens in the managed runtime before the user turn starts).
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { observedRunText, requireUserEnv, runDiagnostics, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: environment.packages is pre-installed on managed runs", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed deepseek pre-installs an apt package (jq) before the agent runs",
    async () => {
      const script = sdkRunnerScript({
        run: `{
          provider: "deepseek",
          model: MODEL_DEEPSEEK,
          message: [
            "Use the bash tool exactly once to run this command without installing anything:",
            "\`command -v jq || echo JQ_MISSING\`",
            "Reply with the exact stdout."
          ],
          environment: { packages: [{ name: "jq" }] },
          apiKeys: { deepseek: DEEPSEEK_KEY },
          idempotencyKey: "user-packages-deepseek-managed-a-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-packages-deepseek-managed-a.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");
      const text = observedRunText(result);
      // Pre-installed: the shell result found the binary without installing it.
      expect(text, runDiagnostics(result)).toContain("/usr/bin/jq");
    },
    10 * 60_000
  );

  it(
    "managed runtime pre-installs apt (jq) AND pip (cowsay) before the agent runs",
    async () => {
      // apt jq (unprefixed → apt) exercises the ROOT-entrypoint apt path.
      // pip:cowsay exercises the system-wide pip path. Both must be present
      // without the agent installing.
      const script = sdkRunnerScript({
        run: `{
          provider: "deepseek",
          model: MODEL_DEEPSEEK,
          message: [
            "Use the bash tool exactly once to run this command without installing anything:",
            "\`command -v jq || echo JQ_MISSING; python3 -c \\"import cowsay; print('PIP_OK')\\" || echo PIP_MISSING\`",
            "Reply with the exact stdout."
          ],
          environment: { packages: [{ name: "jq" }, { name: "pip:cowsay" }] },
          apiKeys: { deepseek: DEEPSEEK_KEY },
          idempotencyKey: "user-packages-managed-runtime-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-packages-managed-runtime.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");
      const text = observedRunText(result);
      // apt: jq pre-installed system-wide by the runtime setup path.
      expect(text, runDiagnostics(result)).toContain("/usr/bin/jq");
      // pip: the python module imports without the agent installing it.
      expect(text, runDiagnostics(result)).toContain("PIP_OK");
    },
    10 * 60_000
  );
});
