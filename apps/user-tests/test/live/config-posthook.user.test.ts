/**
 * USER TEST (SDK-driven) — postHook is removed from the public SDK surface.
 *
 * This is intentionally a live-suite user test even though it must not dispatch
 * a live run: a fresh installed SDK should reject `postHook` before the first
 * HTTP request. Validation scripts that used to be post hooks now belong in a
 * follow-up session/chat message after the turn idles.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("user/SDK: postHook is rejected at the public boundary", () => {
  it("rejects postHook before the SDK sends any HTTP request", async () => {
    const script = `
      import { strictEqual, match } from "node:assert/strict";
      import { Aex } from "@aexhq/sdk";

      const calls = [];
      const client = new Aex({
        baseUrl: "https://example.invalid",
        apiKey: "aex_user_posthook_token",
        fetch: async (input, init) => {
          calls.push({ input: String(input), method: init && init.method });
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
      });

      let message = "";
      try {
        await client.start({
          model: "deepseek-v4-flash",
          message: "This should never be submitted.",
          builtinTools: "none",
          postHook: { command: "bun test" },
          idempotencyKey: "removed-posthook"
        });
      } catch (err) {
        message = err && err.message ? err.message : String(err);
      }

      match(message, /postHook is not a supported option/);
      strictEqual(calls.length, 0);
      process.stdout.write(JSON.stringify({ ok: true, message, calls: calls.length }));
    `;
    const scriptPath = join(install.installDir, "removed-posthook.mjs");
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], {
      cwd: install.installDir,
      timeoutMs: 60_000
    });
    if (child.exitCode !== 0) {
      throw new Error(
        `removed postHook runner exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
      );
    }
    const result = JSON.parse(child.stdout.trim()) as { ok: boolean; calls: number; message: string };
    expect(result.ok).toBe(true);
    expect(result.calls).toBe(0);
    expect(result.message).toMatch(/postHook is not a supported option/);
  });
});
