/**
 * USER TEST (SDK-driven) — environment.envVars is delivered on managed runs.
 *
 * Validates the FIX end-to-end through the installed SDK: a customer-supplied
 * `environment.envVars` value reaches the agent via the managed runtime's
 * `/workspace/RUNTIME.env` mount. The runner does not export the vars into
 * the agent's process env; they are
 * mounted as a file the agent reads. So each test asks the agent to `cat` the
 * runtime-appropriate path and echo back a random per-test canary.
 *
 * Only passes once the fixes are DEPLOYED to the remote aex-local worker
 * (the managed-runtime half also requires the runner image to be rebuilt, since the
 * file is written by the runtime materialization step).
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: environment.envVars reaches the agent on managed runs", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed deepseek delivers envVars via /workspace/RUNTIME.env",
    async () => {
      const canary = "ENVVAR-" + Math.random().toString(36).slice(2, 10);
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: [
            "Using the shell, run exactly: cat /workspace/RUNTIME.env",
            "Reply with ONLY the value of CANARY_VALUE from that file.",
            "If the file or the variable is missing, reply with exactly: CANARY_UNSET"
          ],
          environment: { envVars: { CANARY_VALUE: ${JSON.stringify(canary)} } },
          secrets: { apiKey: DEEPSEEK_KEY },
          idempotencyKey: "user-envvars-deepseek-managed-a-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-envvars-deepseek-managed-a.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");
      // dense() guards against a streamed canary fragmented by spaces.
      expect(dense(result.assistantText)).toContain(canary);
    },
    10 * 60_000
  );
});
