/**
 * USER TEST (SDK-driven) — multiple `agentsMd` refs are ALL delivered.
 *
 * Validates the FIX end-to-end through the installed SDK: when a run ships
 * TWO AgentsMd files, BOTH reach the agent (not just `agentsMd[0]`). Each
 * AGENTS.md states a distinct, benign project fact (a codename); a single
 * question needs both facts, so a correct answer proves both refs landed.
 *
 * Framing note: we use benign "project facts" rather than "echo these
 * tokens". A natural project-doc question is answered normally.
 *
 * Model-cooperation-dependent: if tokenA is ALSO absent the run was
 * inconclusive and that assert fails loudly rather than passing silently.
 *
 * Only passes once the agentsMd multi-ref fix is DEPLOYED to the remote
 * aex-local worker.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

function rand(prefix: string): string {
  return `${prefix}-${Math.random().toString(36).slice(2, 10)}`;
}

const PROMPT =
  "From your project notes, what is the internal codename for this project, and for the database? " +
  "Answer with both. If a codename is not in your notes, write UNKNOWN for it.";

describe("user/SDK: every agentsMd ref reaches the agent (not just the first)", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed deepseek delivers both agentsMd refs",
    async () => {
      const tokenA = rand("ALPHA");
      const tokenB = rand("BRAVO");
      const script = sdkRunnerScript({
        setup: `
          const a = await AgentsMd.fromContent("# Project notes A\\n\\nThe internal codename for this project is ${tokenA}.", { name: "rules-a" });
          const b = await AgentsMd.fromContent("# Project notes B\\n\\nThe internal codename for the database is ${tokenB}.", { name: "rules-b" });
        `,
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: ${JSON.stringify([PROMPT])},
          agentsMd: [a, b],
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-agentsmd-deepseek-managed-a-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-agentsmd-deepseek-managed-a.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");

      const text = dense(result.assistantText);
      // The first ref WAS delivered (proves the mechanism + a conclusive run)...
      expect(text).toContain(tokenA);
      // THE FIX: ...and the second ref reaches the agent too.
      expect(text).toContain(tokenB);
    },
    10 * 60_000
  );

  it(
    "goose (managed) delivers both agentsMd refs",
    async () => {
      const tokenA = rand("ALPHA");
      const tokenB = rand("BRAVO");
      const script = sdkRunnerScript({
        setup: `
          const a = await AgentsMd.fromContent("# Project notes A\\n\\nThe internal codename for this project is ${tokenA}.", { name: "rules-a" });
          const b = await AgentsMd.fromContent("# Project notes B\\n\\nThe internal codename for the database is ${tokenB}.", { name: "rules-b" });
        `,
        submit: `{
          provider: "deepseek",
          runtime: "managed",
          model: MODEL_DEEPSEEK,
          prompt: ${JSON.stringify([PROMPT])},
          agentsMd: [a, b],
          secrets: { deepseek: { apiKey: DEEPSEEK_KEY } },
          idempotencyKey: "user-agentsmd-goose-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-agentsmd-goose.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");

      const text = dense(result.assistantText);
      expect(text).toContain(tokenA);
      expect(text).toContain(tokenB);
    },
    10 * 60_000
  );
});
