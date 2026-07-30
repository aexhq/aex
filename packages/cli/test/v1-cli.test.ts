import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { newId } from "@aexhq/contracts";
import { executeCli } from "../src/main.js";
import { json, makeHarness } from "./support.js";

const API = ["--api-key", "aex_prd_test", "--aex-url", "https://regional.example"];

describe("v1 resource grammar", () => {
  test("legacy commands are removed, not aliased", async () => {
    for (const command of [
      "start", "status", "deliveries", "cancel", "archive", "checkpoint",
      "webhooks", "runtime", "runtime-sizes", "version", "copy", "resume",
      "suspend", "otel", "tail", "inspect", "download", "agents"
    ]) {
      const cap = makeHarness({ args: [command, ...API] });
      await executeCli(cap.io);
      expect(cap.exitCode).toBe(2);
      expect(cap.stderr).toContain(`removed command: ${command}`);
      expect(cap.calls).toHaveLength(0);
    }
  });

  test("help names explicit resources and omits legacy flows", async () => {
    const cap = makeHarness({ args: ["help"] });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("sessions create|list|get|stop|persist|fork|delete");
    expect(cap.stdout).toContain("files live|persisted");
    expect(cap.stdout).not.toContain("aex start");
    expect(cap.stdout).not.toContain("suspend");
  });

  test("legacy cursor, follow, runtime, webhook, and ticket flags are rejected", async () => {
    for (const oldFlag of ["from", "follow", "runtime", "runtime-size", "webhook", "ticket"]) {
      const cap = makeHarness({
        args: ["events", "query", `--${oldFlag}`, "legacy", ...API]
      });
      await executeCli(cap.io);
      expect(cap.exitCode).toBe(2);
      expect(cap.stderr).toContain(`removed flag: --${oldFlag}`);
      expect(cap.calls).toHaveLength(0);
    }
  });

  test("session create forwards canonical JSON and idempotency then writes JSON output", async () => {
    const sessionId = newId("session");
    const request = { model: "openai/gpt-5" };
    const cap = makeHarness({
      args: [
        "sessions", "create", "--request", JSON.stringify(request),
        "--idempotency-key", "create-1", "--output", "created.json", ...API
      ],
      fetch: (call) => {
        expect(call.url).toBe("https://regional.example/api/sessions");
        expect(call.init.method).toBe("POST");
        expect(new Headers(call.init.headers).get("idempotency-key")).toBe("create-1");
        expect(call.body).toEqual(request);
        return json({ id: sessionId, workspaceId: newId("workspace"), status: "idle" }, 201);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toBe("");
    expect(new TextDecoder().decode(cap.writes.get("C:\\cli-test\\created.json"))).toContain(sessionId);
  });

  test("signal query is an observational POST with explicit session scope", async () => {
    const sessionId = newId("session");
    const workspaceId = newId("workspace");
    const query = { limit: 5, order: "asc" };
    const cap = makeHarness({
      args: ["events", "query", "--session", sessionId, "--query", JSON.stringify(query), ...API],
      fetch: (call) => {
        if (call.url.endsWith(`/api/sessions/${sessionId}`)) {
          return json({ id: sessionId, workspaceId, status: "idle" });
        }
        return json({ items: [], coverage: { complete: true } });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(2);
    expect(cap.calls[0]?.init.method ?? "GET").toBe("GET");
    expect(cap.calls[1]?.url).toBe(`https://regional.example/api/sessions/${sessionId}/events/query`);
    expect(cap.calls[1]?.init.method).toBe("POST");
    expect(cap.calls[1]?.body).toEqual(query);
  });

  test("query JSON can come from piped stdin", async () => {
    const query = { limit: 3 };
    const cap = makeHarness({
      args: ["logs", "query", ...API],
      textFiles: { stdin: JSON.stringify(query) },
      stdinIsTTY: false,
      fetch: (call) => {
        expect(call.url).toBe("https://regional.example/api/logs/query");
        expect(call.body).toEqual(query);
        return json({ items: [], coverage: { complete: true } });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
  });

  test("live-file controls remain explicit in the request body", async () => {
    const sessionId = newId("session");
    const workspaceId = newId("workspace");
    const cap = makeHarness({
      args: [
        "files", "live", "stat", sessionId, "state.json",
        "--wake", "retained", "--consistency", "best_effort", ...API
      ],
      fetch: (call) => {
        if (call.url.endsWith(`/api/sessions/${sessionId}`)) {
          return json({ id: sessionId, workspaceId, status: "idle" });
        }
        expect(call.url).toBe(`https://regional.example/api/sessions/${sessionId}/files/live/stat`);
        expect(call.body).toEqual({
          path: "state.json",
          wake: "retained",
          consistency: "best_effort"
        });
        return json({ path: "state.json", sizeBytes: 1 });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
  });

  test("paused API errors remain structured and do not fall through to a mutation", async () => {
    const sessionId = newId("session");
    const cap = makeHarness({
      args: ["sessions", "stop", sessionId, "--operation-id", newId("operation"), ...API],
      fetch: (call) => {
        expect(call.url).toBe(`https://regional.example/api/sessions/${sessionId}/stops`);
        expect(call.init.method).toBe("POST");
        return json({
          error: "account_paused",
          message: "top up required",
          requestId: "req_pause"
        }, 402);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(1);
    expect(JSON.parse(cap.stderr)).toMatchObject({ error: "account_paused" });
    expect(cap.calls).toHaveLength(1);
  });

  test("registered resources are overwrite-only PUTs with revision and idempotency", async () => {
    const value = { source: { type: "inline", text: "hello" } };
    const cap = makeHarness({
      args: [
        "workspace", "files", "set", "notes", "--request", JSON.stringify(value),
        "--if-revision", "7", "--idempotency-key", "overwrite-1", ...API
      ],
      fetch: (call) => {
        expect(call.url).toBe("https://regional.example/api/workspace/files/notes");
        expect(call.init.method).toBe("PUT");
        const headers = new Headers(call.init.headers);
        expect(headers.get("if-match")).toBe('"7"');
        expect(headers.get("idempotency-key")).toBe("overwrite-1");
        expect(call.body).toEqual(value);
        expect(call.url).not.toContain("versions");
        return json({ kind: "file", name: "notes", value, revision: 8 });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(1);
  });
});

describe("durable operations and downloads", () => {
  test("session stop admits directly and waits through the safe operation read", async () => {
    const sessionId = newId("session");
    const workspaceId = newId("workspace");
    const operationId = newId("operation");
    const base = {
      id: operationId,
      workspaceId,
      sessionId,
      kind: "session_stop",
      createdAt: "2026-07-30T00:00:00.000Z",
      updatedAt: "2026-07-30T00:00:00.000Z"
    };
    const cap = makeHarness({
      args: [
        "sessions", "stop", sessionId, "--operation-id", operationId,
        "--poll-interval-ms", "0", ...API
      ],
      fetch: (call) => {
        if (call.url.endsWith(`/api/sessions/${sessionId}/stops`)) {
          return json({ ...base, status: "pending" }, 202);
        }
        return json({ ...base, status: "succeeded", result: {} });
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout).status).toBe("succeeded");
    expect(cap.calls).toHaveLength(2);
    expect(cap.calls[0]?.init.method).toBe("POST");
    expect(cap.calls[1]?.url).toBe(`https://regional.example/api/operations/${operationId}`);
    expect(cap.calls[1]?.init.method ?? "GET").toBe("GET");
    expect(cap.calls.some((call) => call.url.endsWith(`/api/sessions/${sessionId}`))).toBeFalse();
  });

  test("--detach prints the admitted operation without polling", async () => {
    const workspaceId = newId("workspace");
    const operationId = newId("operation");
    const operation = {
      id: operationId,
      workspaceId,
      kind: "workspace_delete",
      status: "pending",
      createdAt: "2026-07-30T00:00:00.000Z",
      updatedAt: "2026-07-30T00:00:00.000Z"
    };
    const cap = makeHarness({
      args: [
        "workspaces", "delete", workspaceId, "--confirm", workspaceId,
        "--operation-id", operationId, "--detach", ...API
      ],
      fetch: (call) => {
        expect(call.url).toBe(`https://regional.example/api/workspaces/${workspaceId}/deletions`);
        expect(new Headers(call.init.headers).get("aex-operation-id")).toBe(operationId);
        return json(operation, 202);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout)).toEqual(operation);
    expect(cap.calls).toHaveLength(1);
  });

  test("persisted download consumes one grant, verifies it, and atomically renames", async () => {
    const sessionId = newId("session");
    const workspaceId = newId("workspace");
    const bytes = new TextEncoder().encode("artifact");
    const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
    let grants = 0;
    const cap = makeHarness({
      args: [
        "files", "persisted", "download", sessionId, "out/result.txt",
        "--output", "result.txt", "--idempotency-key", "download-1", ...API
      ],
      fetch: (call) => {
        if (call.url.endsWith(`/api/sessions/${sessionId}`)) {
          return json({ id: sessionId, workspaceId, status: "idle" });
        }
        if (call.url.endsWith("/files/persisted/downloads")) {
          grants += 1;
          return json({
            url: "https://objects.example/grant",
            expiresAt: "2026-07-30T01:00:00.000Z",
            sizeBytes: bytes.byteLength,
            authorizedBytes: bytes.byteLength,
            measurementId: newId("measurement"),
            sha256: digest
          }, 201);
        }
        return new Response(bytes);
      }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(grants).toBe(1);
    expect(cap.writes.has("C:\\cli-test\\result.txt.part")).toBeFalse();
    expect(cap.writes.get("C:\\cli-test\\result.txt")).toEqual(bytes);
    expect(cap.stdout).toBe("");
    expect(cap.stderr).not.toContain("objects.example");
    expect(cap.calls).toHaveLength(3);
    expect(cap.calls[1]?.body).toEqual({ path: "out/result.txt" });
    expect(new Headers(cap.calls[1]?.init.headers).get("idempotency-key")).toBe("download-1");
    expect(cap.calls[2]?.url).toBe("https://objects.example/grant");
  });

  test("an existing output fails before a download grant is minted", async () => {
    const cap = makeHarness({
      args: [
        "files", "live", "download", newId("session"), "result.txt",
        "--output", "result.txt", "--wake", "never", "--consistency", "coherent", ...API
      ],
      byteFiles: { "C:\\cli-test\\result.txt": new Uint8Array([1]) }
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("output already exists");
    expect(cap.calls).toHaveLength(0);
  });

  test("binary stdout contains bytes only", async () => {
    const organizationId = newId("organization");
    const statementId = newId("statement");
    const bytes = new Uint8Array([0, 1, 2, 255]);
    const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
    const cap = makeHarness({
      args: [
        "billing", "statements", "download", statementId,
        "--organization", organizationId, "--output", "-",
        "--idempotency-key", "statement-1", ...API
      ],
      fetch: (call) => call.url.includes("/downloads")
        ? json({
          url: "https://objects.example/statement",
          expiresAt: "2026-07-30T01:00:00.000Z",
          sizeBytes: bytes.byteLength,
          authorizedBytes: bytes.byteLength,
          measurementId: newId("measurement"),
          sha256: digest
        }, 201)
        : new Response(bytes)
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toBe("");
    expect(cap.stdoutBytes).toEqual(bytes);
    expect(cap.stderr).toBe("");
  });
});
