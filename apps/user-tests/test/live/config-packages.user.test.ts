/**
 * USER TEST (SDK-driven) — environment.packages pre-installs on both runtimes.
 *
 * Validates the FIX end-to-end through the installed SDK: a customer-supplied
 * `environment.packages` entry is PRE-INSTALLED before the agent runs, so the
 * agent finds it WITHOUT installing anything itself —
 *   - native: an unprefixed name ("jq") installs via apt; the agent confirms
 *     the binary is on PATH (`which jq` → /usr/bin/jq).
 *   - goose:  validates BOTH apt and pip on managed — an unprefixed "jq"
 *     (→ apt) AND a "pip:cowsay" (→ pip). The ROOT entrypoint installs apt
 *     system-wide (parity with native) and pip system-wide; the agent confirms
 *     `which jq` → /usr/bin/jq AND that the python `cowsay` module imports.
 *
 * Only passes once the fixes are DEPLOYED to the remote antpath-local worker
 * (the goose half ALSO requires the runner image to be rebuilt, since package
 * pre-install now happens in the managed runtime before the user turn starts).
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ anthropic: true, deepseek: true });

describe("user/SDK: environment.packages is pre-installed on both runtimes", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "native pre-installs an apt package (jq) before the agent runs",
    async () => {
      // jq (not cowsay): verified live that the Anthropic environment
      // preinstalls jq to /usr/bin/jq (on PATH). cowsay isn't in the base
      // image's default apt sources and installs to /usr/games (off PATH).
      const script = sdkRunnerScript({
        submit: `{
          provider: "anthropic",
          runtime: "native",
          model: MODEL_ANTHROPIC,
          prompt: [
            "Without installing anything, run \`which jq\` via the shell;",
            "reply with the path, or JQ_MISSING if absent."
          ],
          environment: { packages: [{ name: "jq" }] },
          secrets: { anthropic: { apiKey: ANTHROPIC_KEY } },
          idempotencyKey: "user-packages-native-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-packages-native.mjs",
        waitMs: 4 * 60_000,
        timeoutMs: 5 * 60_000
      });

      expect(result.runtime).toBe("native");
      expect(result.status).toBe("succeeded");
      const text = dense(result.assistantText);
      // Pre-installed: the agent found the binary without installing it.
      expect(text).not.toContain("JQ_MISSING");
      expect(text).toContain("/usr/bin/jq");
    },
    6 * 60_000
  );

  it(
    "goose (managed) pre-installs apt (jq) AND pip (cowsay) before the agent runs",
    async () => {
      // apt jq (unprefixed → apt) exercises the ROOT-entrypoint apt path —
      // parity with the native apt path, jq at /usr/bin/jq. pip:cowsay exercises
      // the system-wide pip path. Both must be present without the agent installing.
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
      // apt: jq pre-installed system-wide by the ROOT entrypoint (parity w/ native).
      expect(text).not.toContain("JQ_MISSING");
      expect(text).toContain("/usr/bin/jq");
      // pip: the python module imports without the agent installing it.
      expect(text).not.toContain("PIP_MISSING");
      expect(text).toContain("PIP_OK");
    },
    10 * 60_000
  );
});
