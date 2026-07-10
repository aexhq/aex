import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { getBunCommand, runCommand } from "../_fixtures/install.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

function extractCorruptedSkillRunnerScript(): string {
  const source = readFileSync(
    resolve(repoRoot, "apps/user-tests/test/live/live-sdk-files-and-failures.test.ts"),
    "utf8"
  ).replace(/\r\n/g, "\n");
  const functionStart = source.indexOf("function buildCorruptedSkillScript");
  const nextFunction = source.indexOf("function buildIncompatibleRuntimeScript", functionStart);
  const returnStart = source.indexOf("return `\n", functionStart);
  const returnEnd = source.lastIndexOf("\n  `;", nextFunction);
  if (functionStart < 0 || nextFunction < 0 || returnStart < 0 || returnEnd < 0) {
    throw new Error("could not extract buildCorruptedSkillScript body");
  }

  const script = source.slice(returnStart + "return `\n".length, returnEnd);
  const modelNeedle = "${JSON.stringify(deepseekModel)}";
  if (!script.includes(modelNeedle)) {
    throw new Error("corrupted-skill runner no longer contains the model interpolation");
  }
  return script.replace(modelNeedle, JSON.stringify("deepseek-v4-flash"));
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

async function drain(req: IncomingMessage): Promise<void> {
  for await (const _chunk of req) {
    // Drain request bodies so the child script sees completed responses.
  }
}

async function listen(server: ReturnType<typeof createServer>): Promise<string> {
  await new Promise<void>((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address() as AddressInfo;
  return `http://127.0.0.1:${address.port}`;
}

async function close(server: ReturnType<typeof createServer>): Promise<void> {
  await new Promise<void>((resolveClose, rejectClose) => {
    server.close((error) => (error ? rejectClose(error) : resolveClose()));
  });
}

describe("corrupted skill failure live runner", () => {
  it("polls an accepted session to terminal failure before reporting the failure contract", async () => {
    const sessionId = "ses_fake_corrupted_skill";
    const requests: string[] = [];
    let origin = "";
    let runPolls = 0;
    const server = createServer((req, res) => {
      void (async () => {
        const method = req.method ?? "GET";
        const url = new URL(req.url ?? "/", "http://127.0.0.1");
        requests.push(`${method} ${url.pathname}${url.search}`);

        if (method === "POST" && url.pathname === "/assets/presign") {
          await drain(req);
          json(res, 200, { uploadUrl: `${origin}/upload/corrupt-skill.zip`, requiredHeaders: {} });
          return;
        }
        if (method === "PUT" && url.pathname === "/upload/corrupt-skill.zip") {
          await drain(req);
          res.writeHead(200);
          res.end();
          return;
        }
        if (method === "POST" && url.pathname === "/assets/finalize") {
          await drain(req);
          json(res, 200, { ok: true });
          return;
        }
        if (method === "PUT" && url.pathname === "/api/skills/corrupt-skill") {
          await drain(req);
          json(res, 200, { ok: true });
          return;
        }
        if (method === "POST" && url.pathname === "/api/sessions") {
          await drain(req);
          json(res, 202, { session: { id: sessionId, sessionId } });
          return;
        }
        if (method === "POST" && url.pathname === `/api/sessions/${sessionId}/messages`) {
          await drain(req);
          json(res, 202, { session: { id: sessionId, sessionId }, turn: { sessionId, turnSeq: 1 } });
          return;
        }
        if (method === "GET" && url.pathname === `/api/sessions/${sessionId}`) {
          runPolls += 1;
          if (runPolls === 1) {
            json(res, 200, { id: sessionId, status: "running" });
            return;
          }
          json(res, 200, {
            id: sessionId,
            status: "failed",
            errorMessage:
              "BootMaterializeError: boot materialize failed: materialize failed: skill bundle 'corrupt-skill' is unmaterializable (missing_SKILL.md)",
            failureClass: "setup_failed"
          });
          return;
        }
        if (method === "GET" && url.pathname === `/api/sessions/${sessionId}/events`) {
          json(res, 200, {
            events: [
              { type: "TURN_STARTED", sessionId },
              {
                type: "TURN_ERROR",
                sessionId,
                data: {
                  reason: "failed",
                  failureClass: "setup_failed",
                  failureMessage:
                    "BootMaterializeError: boot materialize failed: materialize failed: skill bundle 'corrupt-skill' is unmaterializable (missing_SKILL.md)"
                }
              }
            ]
          });
          return;
        }

        await drain(req);
        json(res, 404, { error: "not_found", path: url.pathname });
      })().catch((error: unknown) => {
        res.writeHead(500, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) }));
      });
    });

    const dir = mkdtempSync(join(tmpdir(), "aex-corrupt-skill-runner-"));
    try {
      origin = await listen(server);
      const scriptPath = join(dir, "corrupted-skill-runner.mjs");
      writeFileSync(scriptPath, extractCorruptedSkillRunnerScript());

      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: dir,
        timeoutMs: 20_000,
        env: {
          AEX_API_URL: origin,
          AEX_API_KEY: "aex_fake_key",
          DEEPSEEK_KEY_SUBMIT: "deepseek_fake_key",
          FAILURE_WAIT_MS: "5000"
        }
      });

      expect(child.exitCode, child.stderr).toBe(0);
      const result = JSON.parse(child.stdout) as {
        readonly submitOk: boolean;
        readonly submitStatus: number;
        readonly sessionId: string | null;
        readonly sessionStatus: string | null;
        readonly sessionPollStatus: number | null;
        readonly sessionPollAttempts: number;
        readonly sessionErrorMessage: string | null;
        readonly terminalKind: string | null;
        readonly terminalData: Record<string, unknown> | null;
        readonly eventKinds: readonly string[];
      };

      expect(result.submitOk).toBe(true);
      expect(result.submitStatus).toBe(202);
      expect(result.sessionId).toBe(sessionId);
      expect(result.sessionStatus).toBe("failed");
      expect(result.sessionPollStatus).toBe(200);
      expect(result.sessionPollAttempts).toBeGreaterThanOrEqual(2);
      expect(result.sessionErrorMessage).toContain("BootMaterializeError");
      expect(result.terminalKind).toBe("TURN_ERROR");
      expect(result.terminalData?.["failureClass"]).toBe("setup_failed");
      expect(result.eventKinds).toContain("TURN_ERROR");
      expect(requests).toContain(`POST /api/sessions/${sessionId}/messages`);
      expect(requests).toContain(`GET /api/sessions/${sessionId}`);
      expect(requests).toContain(`GET /api/sessions/${sessionId}/events?limit=1000`);
    } finally {
      await close(server).catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
