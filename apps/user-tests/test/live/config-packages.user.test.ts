/**
 * USER TEST (SDK-driven) — environment.packages pre-installs on managed runs.
 *
 * Validates the FIX end-to-end through the installed SDK: a customer-supplied
 * `environment.packages` entries are PRE-INSTALLED before the agent runs, so
 * the agent finds them WITHOUT installing anything itself. The managed runner
 * validates BOTH apt and pip — an unprefixed "jq" (→ apt) AND a "pip:cowsay"
 * (→ pip).
 *
 * Only passes once the fixes are DEPLOYED to the remote antpath-local worker
 * (the goose half ALSO requires the runner image to be rebuilt, since package
 * pre-install now happens in the managed runtime before the user turn starts).
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

describe("user/SDK: environment.packages is pre-installed on managed runs", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed deepseek pre-installs an apt package (jq) before the agent runs",
    async () => {
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: [
            "Without installing anything, run \`which jq\` via the shell;",
            "reply with the path, or JQ_MISSING if absent."
          ],
          environment: { packages: [{ name: "jq" }] },
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
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
      const text = dense(result.assistantText);
      // Pre-installed: the agent found the binary without installing it.
      expect(text).not.toContain("JQ_MISSING");
      expect(text).toContain("/usr/bin/jq");
    },
    10 * 60_000
  );

  it(
    "goose (managed) pre-installs apt (jq) AND pip (cowsay) before the agent runs",
    async () => {
      // apt jq (unprefixed → apt) exercises the ROOT-entrypoint apt path.
      // pip:cowsay exercises the system-wide pip path. Both must be present
      // without the agent installing.
      const script = sdkRunnerScript({
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: [
            "Without installing anything, run BOTH of these via the shell and report each result:",
            "1) \`which jq\` — reply with the path, or JQ_MISSING if absent.",
            "2) \`python3 -c \\"import cowsay; print('PIP_OK')\\"\` — reply with its output, or PIP_MISSING if the import fails."
          ],
          environment: { packages: [{ name: "jq" }, { name: "pip:cowsay" }] },
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-packages-goose-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-packages-goose.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");
      const text = dense(result.assistantText);
      // apt: jq pre-installed system-wide by the ROOT entrypoint.
      expect(text).not.toContain("JQ_MISSING");
      expect(text).toContain("/usr/bin/jq");
      // pip: the python module imports without the agent installing it.
      expect(text).not.toContain("PIP_MISSING");
      expect(text).toContain("PIP_OK");
    },
    10 * 60_000
  );
});
