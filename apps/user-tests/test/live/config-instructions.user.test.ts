/**
 * USER TEST (SDK-driven) — multiple `instructions` refs are ALL delivered.
 *
 * Validates the FIX end-to-end through the installed SDK: when a session ships
 * TWO instruction resources, BOTH reach the agent. Each resource states a
 * distinct, benign project fact (a codename); a single
 * question needs both facts, so a correct answer proves both refs landed.
 *
 * Framing note: we use benign "project facts" rather than "echo these
 * tokens". A natural project-doc question is answered normally.
 *
 * Model-cooperation-dependent: if tokenA is ALSO absent the session was
 * inconclusive and that assert fails loudly rather than passing silently.
 *
 * Only passes once the instructions multi-ref fix is DEPLOYED to the remote
 * dev worker.
 */
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { installAex, type InstallResult } from "../_fixtures/install.js";
import { dense, requireUserEnv, runSdkScript, sdkRunnerScript } from "./_sdk.js";

const env = requireUserEnv({ deepseek: true });

function rand(prefix: string): string {
  return `${prefix}-${Math.random().toString(36).slice(2, 10)}`;
}

const PROMPT =
  "From your project notes, what is the internal codename for this project, and for the database? " +
  "Answer with both. If a codename is not in your notes, write UNKNOWN for it.";

describe("user/SDK: every instructions ref reaches the agent (not just the first)", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  it(
    "managed deepseek delivers both instructions refs",
    async () => {
      const tokenA = rand("ALPHA");
      const tokenB = rand("BRAVO");
      // The high-entropy suffix is the delivery proof; the "ALPHA-"/"BRAVO-"
      // prefix is a human label the model may reformat away.
      const markerA = tokenA.slice(tokenA.indexOf("-") + 1);
      const markerB = tokenB.slice(tokenB.indexOf("-") + 1);
      const script = sdkRunnerScript({
        setup: `
          const aDraft = await Instructions.fromContent("# Project notes A\\n\\nThe internal codename for this project is ${tokenA}.", { name: "rules-a" });
          const bDraft = await Instructions.fromContent("# Project notes B\\n\\nThe internal codename for the database is ${tokenB}.", { name: "rules-b" });
          const a = await client.workspace.instructions.publish(aDraft);
          const b = await client.workspace.instructions.publish(bDraft);
        `,
        session: `{
          provider: "deepseek",
          model: MODEL_DEEPSEEK,
          message: ${JSON.stringify([PROMPT])},
          assets: { instructions: [a, b] },
          apiKeys: { deepseek: DEEPSEEK_KEY },
          idempotencyKey: "user-instructions-deepseek-managed-a-" + Date.now()
        }`
      });
      const result = await runSdkScript(install, env, script, {
        scriptName: "user-instructions-deepseek-managed-a.mjs",
        waitMs: 8 * 60_000,
        timeoutMs: 9 * 60_000
      });

      expect(result.runtime).toBe("managed");
      expect(result.status).toBe("succeeded");

      const text = dense(result.assistantText);
      // The first ref WAS delivered (proves the mechanism + a conclusive run)...
      expect(text).toContain(markerA);
      // THE FIX: ...and the second ref reaches the agent too.
      expect(text).toContain(markerB);
    },
    10 * 60_000
  );
});
