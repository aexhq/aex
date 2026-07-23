import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createHash } from "node:crypto";
import type { AddressInfo } from "node:net";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "bun:test";
import { getBunCommand, runCommand } from "../_fixtures/install.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

function extractCorruptedSkillRunnerScript(): string {
  const path = resolve(repoRoot, "apps/user-tests/test/live/live-sdk-files-and-failures.test.ts");
  const text = readFileSync(
    path,
    "utf8"
  ).replace(/\r\n/g, "\n");
  const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  let template: ts.TemplateExpression | undefined;
  const visit = (node: ts.Node): void => {
    if (
      ts.isFunctionDeclaration(node) &&
      node.name?.text === "buildCorruptedSkillScript" &&
      node.body
    ) {
      for (const statement of node.body.statements) {
        if (ts.isReturnStatement(statement) && statement.expression && ts.isTemplateExpression(statement.expression)) {
          template = statement.expression;
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  if (!template) throw new Error("could not find the corrupted-skill runner template");

  let script = template.head.text;
  for (const span of template.templateSpans) {
    if (span.expression.getText(source) !== "JSON.stringify(deepseekModel)") {
      throw new Error(`unsupported corrupted-skill runner interpolation: ${span.expression.getText(source)}`);
    }
    script += JSON.stringify("deepseek-v4-flash");
    script += span.literal.text;
  }
  return script;
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

async function readJson(req: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  return JSON.parse(Buffer.concat(chunks).toString("utf8")) as Record<string, unknown>;
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
  it("waits for RUN_ERROR before reporting the resumable session's error state", async () => {
    const sessionId = "ses_fake_corrupted_skill";
    const requests: string[] = [];
    let origin = "";
    const corruptedZip = new Uint8Array([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    const hashHex = createHash("sha256").update(corruptedZip).digest("hex");
    const contentHash = `sha256:${hashHex}`;
    const assetId = `asset_${hashHex}`;
    const runId = "run_corrupted_skill";
    const server = createServer((req, res) => {
      void (async () => {
        const method = req.method ?? "GET";
        const url = new URL(req.url ?? "/", "http://127.0.0.1");
        requests.push(`${method} ${url.pathname}${url.search}`);

        if (method === "POST" && url.pathname === "/api/assets/presign") {
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
        if (method === "POST" && url.pathname === "/api/assets/finalize") {
          await drain(req);
          json(res, 200, { ok: true });
          return;
        }
        if (method === "POST" && url.pathname === "/api/workspace/skills") {
          const body = await readJson(req);
          json(res, 201, {
            resource: {
              kind: "skill",
              resourceId: `wres_${"1".repeat(32)}`,
              version: 1,
              assetId,
              contentHash,
              name: body["name"],
              description: body["description"],
              sizeBytes: corruptedZip.byteLength,
              contentType: "application/zip",
              createdAt: "2026-07-10T00:00:00.000Z"
            }
          });
          return;
        }
        if (method === "POST" && url.pathname === "/api/sessions") {
          await drain(req);
          json(res, 201, { session: { id: sessionId, status: "idle", acceptsMessages: true } });
          return;
        }
        if (method === "POST" && url.pathname === `/api/sessions/${sessionId}/messages`) {
          await drain(req);
          json(res, 202, {
            session: { id: sessionId, status: "running", acceptsMessages: false },
            run: { sessionId, runId, turnSeq: 1, eventCursor: 0, phase: "running" }
          });
          return;
        }
        if (method === "GET" && url.pathname === `/api/sessions/${sessionId}`) {
          json(res, 200, {
            session: {
              id: sessionId,
              status: "error",
              acceptsMessages: true,
              lastRun: { runId, outcome: "failed" },
              errorMessage:
                "BootMaterializeError: boot materialize failed: materialize failed: skill bundle 'corrupt-skill' is unmaterializable (missing_SKILL.md)",
              failureClass: "setup_failed"
            }
          });
          return;
        }
        if (method === "GET" && url.pathname === `/api/sessions/${sessionId}/events`) {
          json(res, 200, {
            events: [
              { type: "RUN_STARTED", threadId: sessionId, runId, data: {} },
              {
                type: "RUN_ERROR",
                threadId: sessionId,
                runId,
                data: {
                  outcome: "failed",
                  costUsd: 0,
                  providerUsage: [],
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
      expect(result.sessionStatus).toBe("error");
      expect(result.sessionPollStatus).toBe(200);
      expect(result.sessionPollAttempts).toBe(1);
      expect(result.sessionErrorMessage).toContain("BootMaterializeError");
      expect(result.terminalKind).toBe("RUN_ERROR");
      expect(result.terminalData?.["outcome"]).toBe("failed");
      expect(result.terminalData?.["failureClass"]).toBe("setup_failed");
      expect(result.eventKinds).toContain("RUN_ERROR");
      expect(requests).toContain(`POST /api/sessions/${sessionId}/messages`);
      expect(requests).toContain("POST /api/workspace/skills");
      expect(requests).toContain(`GET /api/sessions/${sessionId}`);
      expect(requests).toContain(`GET /api/sessions/${sessionId}/events?limit=1000`);
    } finally {
      await close(server).catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
