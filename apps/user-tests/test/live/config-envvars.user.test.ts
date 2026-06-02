/**
 * USER TEST (SDK-driven) — environment.envVars is delivered on BOTH runtimes.
 *
 * Validates the FIX end-to-end through the installed SDK: a customer-supplied
 * `environment.envVars` value reaches the agent on EACH runtime via that
 * runtime's RUNTIME.env mount —
 *   - native (Anthropic Managed Agents): /mnt/session/uploads/antpath/RUNTIME.env
 *   - goose (managed runner image):       /workspace/RUNTIME.env
 * Neither runtime exports the vars into the agent's process env; they are
 * mounted as a file the agent reads. So each test asks the agent to `cat` the
 * runtime-appropriate path and echo back a random per-test canary.
 *
 * Only passes once the fixes are DEPLOYED to the remote antpath-local worker
 * (the goose half also requires the runner image to be rebuilt, since the
 * file is written by the runtime materialization step).
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ anthropic: true, deepseek: true });

describe("user/SDK: environment.envVars reaches the agent on both runtimes", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "native delivers envVars via /mnt/session/uploads/antpath/RUNTIME.env",
    async () => {
      const canary = "ENVVAR-" + Math.random().toString(36).slice(2, 10);
      const script = sdkRunnerScript({
        submit: `{
          provider: "anthropic",
          runtime: "native",
          model: MODEL_ANTHROPIC,
          prompt: [
            "Using the bash tool, run exactly: cat /mnt/session/uploads/antpath/RUNTIME.env",
            "Reply with ONLY the value of CANARY_VALUE from that file.",
            "If the file or the variable is missing, reply with exactly: CANARY_UNSET"
          ],
          environment: { envVars: { CANARY_VALUE: ${JSON.stringify(canary)} } },
          secrets: { anthropic: { apiKey: ANTHROPIC_KEY } },
          idempotencyKey: "user-envvars-native-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-envvars-native.mjs",
        waitMs: 4 * 60_000,
        timeoutMs: 5 * 60_000
      });

      expect(result.runtime).toBe("native");
      expect(result.status).toBe("succeeded");
      // dense() guards against a streamed canary fragmented by spaces.
      expect(dense(result.assistantText)).toContain(canary);
    },
    6 * 60_000
  );

  it(
    "goose (managed) delivers envVars via /workspace/RUNTIME.env",
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
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-envvars-goose-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-envvars-goose.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");
      expect(dense(result.assistantText)).toContain(canary);
    },
    10 * 60_000
  );
});
