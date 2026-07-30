import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { LIVE_V1_SCENARIOS } from "../_fixtures/live-scenario-ledger.js";
import {
  getBunCommand,
  installAex,
  runCommand,
  type InstallResult
} from "../_fixtures/install.js";

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 300_000);

afterAll(() => {
  install.cleanup();
});

describe("dev strict v1 smoke", () => {
  it("runs the declared explicit session scenario through the clean-installed SDK", async () => {
    expect(LIVE_V1_SCENARIOS.explicitSessionRoundTrip.file).toBe(
      "test/live/v1-session.user.test.ts"
    );
    const script = join(install.installDir, "live-v1.mjs");
    writeFileSync(script, LIVE_SCRIPT);
    const result = await runCommand(getBunCommand(), [script], {
      cwd: install.installDir,
      env: {
        ...process.env,
        AEX_API_URL: process.env.AEX_API_URL,
        AEX_API_KEY: process.env.AEX_API_KEY
      },
      timeoutMs: 600_000
    });
    expect(result.exitCode, result.stderr).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual({
      runStatus: "succeeded",
      deletedSession: true
    });
  }, 610_000);
});

const LIVE_SCRIPT = String.raw`
import { Aex } from "@aexhq/sdk";

if (!process.env.AEX_API_URL || !process.env.AEX_API_KEY) {
  throw new Error("AEX_API_URL and AEX_API_KEY are required for the dev live scenario");
}
const aex = new Aex({
  apiKey: process.env.AEX_API_KEY,
  baseUrl: process.env.AEX_API_URL
});
const session = await aex.sessions.create({
  model: process.env.AEX_USER_TEST_MODEL ?? "anthropic/claude-haiku-4-5"
});
const accepted = await session.messages.send(
  "Reply with exactly: strict-v1-ok",
  { idempotencyKey: "live-v1-message" }
);
const run = await accepted.run.result({ pollIntervalMs: 1_000, timeoutMs: 300_000 });
const deletion = await session.delete({ cascade: true });
const tombstone = await deletion.result({ pollIntervalMs: 1_000, timeoutMs: 300_000 });
process.stdout.write(JSON.stringify({
  runStatus: run.status,
  deletedSession: tombstone.sessionId === session.id
}));
`;
