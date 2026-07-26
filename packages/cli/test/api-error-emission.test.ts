import { readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { AexApiError } from "@aexhq/contracts";
import { describe, expect, it } from "bun:test";
import type { CliIO } from "../src/internal.js";
import {
  RUNTIME_ERR,
  emitApiError,
  type ApiErrorDetails,
  type ApiErrorEmissionOptions
} from "../src/host/common.js";

const here = dirname(fileURLToPath(import.meta.url));
const hostSourceRoot = resolve(here, "..", "src", "host");

function captureIo(): { readonly io: CliIO; readonly stdout: () => string; readonly stderr: () => string } {
  let stdout = "";
  let stderr = "";
  return {
    io: {
      argv: [],
      readFile: async () => "",
      writeFile: async () => undefined,
      cwd: () => "/tmp",
      fetchImpl: fetch,
      stdout: (chunk) => { stdout += chunk; },
      stderr: (chunk) => { stderr += chunk; },
      exit: () => undefined
    },
    stdout: () => stdout,
    stderr: () => stderr
  };
}

describe("emitApiError", () => {
  it("preserves exact stderr JSON bytes, key order, command details, and runtime exit semantics", () => {
    const cap = captureIo();
    const error = new AexApiError(403, "insufficient_scope: denied", {
      error: "insufficient_scope",
      message: "denied"
    });

    const exit = emitApiError(cap.io, "tail_failed", error, {
      sessionId: "session-1",
      lastSeq: 7
    });

    expect(exit).toBe(RUNTIME_ERR);
    expect(cap.stdout()).toBe("");
    expect(cap.stderr()).toBe(
      '{"error":"tail_failed","message":"insufficient_scope: denied","sessionId":"session-1","lastSeq":7,"status":403,"remedy":"token lacks permission for this workspace/action"}\n'
    );
  });

  it("omits optional status/remedy without changing the base envelope", () => {
    const cap = captureIo();
    const exit = emitApiError(cap.io, "session_failed", new Error("submission exploded"));

    expect(exit).toBe(RUNTIME_ERR);
    expect(cap.stdout()).toBe("");
    expect(cap.stderr()).toBe('{"error":"session_failed","message":"submission exploded"}\n');
  });

  it("prefixes the safely described message without losing API diagnostics", () => {
    const cap = captureIo();
    const error = new AexApiError(404, "session not found", {
      error: "session_not_found",
      message: "session not found",
      requestId: "req-4"
    });

    const exit = emitApiError(
      cap.io,
      "session_failed",
      error,
      { sessionId: "session-4" },
      { messagePrefix: "final status fetch failed: " }
    );

    expect(exit).toBe(RUNTIME_ERR);
    expect(cap.stdout()).toBe("");
    expect(cap.stderr()).toBe(
      '{"error":"session_failed","message":"final status fetch failed: session not found — {\\"requestId\\":\\"req-4\\"}","sessionId":"session-4","status":404,"remedy":"no such run/resource — verify the id"}\n'
    );
  });

  it("does not fabricate status or remedy when prefixing a generic error", () => {
    const cap = captureIo();
    const exit = emitApiError(
      cap.io,
      "tail_failed",
      new Error("socket unavailable"),
      { sessionId: "session-1", lastSeq: 3 },
      { messagePrefix: "final status fetch failed: " }
    );

    expect(exit).toBe(RUNTIME_ERR);
    expect(JSON.parse(cap.stderr())).toEqual({
      error: "tail_failed",
      message: "final status fetch failed: socket unavailable",
      sessionId: "session-1",
      lastSeq: 3
    });
  });

  it("types command context without permitting centrally owned envelope keys", () => {
    const details: ApiErrorDetails = { sessionId: "session-1", lastSeq: 2 };
    const options: ApiErrorEmissionOptions = { messagePrefix: "context: " };
    expect(details).toEqual({ sessionId: "session-1", lastSeq: 2 });
    expect(options).toEqual({ messagePrefix: "context: " });

    if (false) {
      const cap = captureIo();
      // @ts-expect-error status is derived from describeApiError, never supplied by a command.
      emitApiError(cap.io, "failed", new Error("x"), { status: 418 });
      // @ts-expect-error remedy is derived from describeApiError, never supplied by a command.
      emitApiError(cap.io, "failed", new Error("x"), { remedy: "override" });
      // @ts-expect-error the command-specific error code is a positional argument.
      emitApiError(cap.io, "failed", new Error("x"), { error: "override" });
      // @ts-expect-error the described message is centrally owned.
      emitApiError(cap.io, "failed", new Error("x"), { message: "override" });
    }
  });
});

const COMMAND_ERROR_MATRIX = [
  { source: "auth-cmd.ts", path: "login workspace whoami", call: 'emitApiError(io, "login_failed", err)' },
  { source: "auth-cmd.ts", path: "login account whoami", call: 'emitApiError(io, "login_failed", err)' },
  { source: "billing.ts", path: "billing show", call: 'emitApiError(io, "billing_failed", err)' },
  { source: "billing.ts", path: "billing portal", call: 'emitApiError(io, "billing_portal_failed", err)' },
  { source: "billing.ts", path: "billing ledger", call: 'emitApiError(io, "billing_ledger_failed", err)' },
  { source: "cancel.ts", path: "cancel", call: 'emitApiError(io, "cancel_failed", err, { sessionId })' },
  { source: "delete-asset.ts", path: "delete-asset", call: 'emitApiError(io, "delete_asset_failed", err, { hash })' },
  { source: "delete.ts", path: "delete", call: 'emitApiError(io, "delete_failed", err, { sessionId })' },
  { source: "deliveries.ts", path: "deliveries", call: 'emitApiError(io, "deliveries_failed", err, { sessionId })' },
  { source: "download.ts", path: "download API read", call: 'emitApiError(io, "download_failed", err, { sessionId })' },
  { source: "events.ts", path: "events initial list", call: 'emitApiError(io, "events_failed", err, { sessionId })' },
  { source: "events.ts", path: "events follow poll", call: 'emitApiError(io, "events_failed", err, { sessionId })' },
  { source: "otel.ts", path: "OTLP export", call: 'emitApiError(io, "otel_failed", err, { sessionId, signal })' },
  { source: "files.ts", path: "files list", call: 'emitApiError(io, "files_failed", err, { sessionId })' },
  { source: "files.ts", path: "files read", call: 'emitApiError(io, "files_read_failed", err, { sessionId, path: selector })' },
  { source: "files.ts", path: "files download API read", call: 'emitApiError(io, "files_download_failed", err, { sessionId, path: selector })' },
  { source: "files.ts", path: "files link", call: 'emitApiError(io, "files_link_failed", err, { sessionId, path: selector })' },
  { source: "files.ts", path: "files find", call: 'emitApiError(io, "files_find_failed", err2, { sessionId })' },
  { source: "inspect.ts", path: "inspect header read", call: 'emitApiError(io, "inspect_failed", err, { sessionId })' },
  { source: "inspect.ts", path: "inspect event poll", call: 'emitApiError(io, "inspect_failed", err, { sessionId })' },
  { source: "keys-cmd.ts", path: "keys list", call: 'emitApiError(io, "keys_list_failed", err)' },
  { source: "keys-cmd.ts", path: "keys create", call: 'emitApiError(io, "key_create_failed", err)' },
  { source: "keys-cmd.ts", path: "keys delete", call: 'emitApiError(io, "key_delete_failed", err)' },
  { source: "list-cmds.ts", path: "sessions", call: 'emitApiError(io, "sessions_failed", err)' },
  { source: "orgs-cmd.ts", path: "orgs list", call: 'emitApiError(io, "orgs_list_failed", err)' },
  { source: "orgs-cmd.ts", path: "orgs create", call: 'emitApiError(io, "org_create_failed", err)' },
  { source: "orgs-cmd.ts", path: "orgs members", call: 'emitApiError(io, "org_members_failed", err)' },
  { source: "orgs-cmd.ts", path: "orgs invite", call: 'emitApiError(io, "org_invite_failed", err)' },
  { source: "start-cmd.ts", path: "start submit", call: 'emitApiError(io, "session_failed", err)' },
  { source: "start-cmd.ts", path: "start final status", call: 'emitApiError(io, "session_failed", err, { sessionId: session.id }, { messagePrefix: "final status fetch failed: " })' },
  { source: "status.ts", path: "status", call: 'emitApiError(io, "status_failed", err, { sessionId })' },
  { source: "tail.ts", path: "tail header read", call: 'emitApiError(io, "tail_failed", err, { sessionId })' },
  { source: "tail.ts", path: "tail event poll", call: 'emitApiError(io, "tail_failed", err, { sessionId, lastSeq })' },
  { source: "tail.ts", path: "tail final status", call: 'emitApiError(io, "tail_failed", err, { sessionId, lastSeq }, { messagePrefix: "final status fetch failed: " })' },
  { source: "wait.ts", path: "wait poll", call: 'emitApiError(io, "wait_failed", err, { sessionId })' },
  { source: "webhooks-cmd.ts", path: "webhooks secret", call: 'emitApiError(io, "webhooks_secret_failed", err)' },
  { source: "whoami.ts", path: "whoami", call: 'emitApiError(io, "whoami_failed", err)' },
  { source: "workspaces-cmd.ts", path: "workspaces list", call: 'emitApiError(io, "workspaces_list_failed", err)' },
  { source: "workspaces-cmd.ts", path: "workspaces create", call: 'emitApiError(io, "workspace_create_failed", err)' },
  { source: "workspaces-cmd.ts", path: "workspaces delete", call: 'emitApiError(io, "workspace_delete_failed", err)' }
] as const;

describe("SDK/API error source ownership", () => {
  it("covers all 40 current command failure paths through the common emitter", () => {
    expect(COMMAND_ERROR_MATRIX).toHaveLength(40);
    const expectedCalls = new Map<string, number>();
    for (const row of COMMAND_ERROR_MATRIX) {
      const key = `${row.source}\0${row.call}`;
      expectedCalls.set(key, (expectedCalls.get(key) ?? 0) + 1);
    }

    for (const [key, expectedCount] of expectedCalls) {
      const [sourceName, call] = key.split("\0") as [string, string];
      const source = readFileSync(resolve(hostSourceRoot, sourceName), "utf8");
      const actualCount = source.split(call).length - 1;
      const paths = COMMAND_ERROR_MATRIX
        .filter((row) => row.source === sourceName && row.call === call)
        .map((row) => row.path)
        .join(", ");
      expect(actualCount, `${sourceName} must preserve ${paths}`).toBe(expectedCount);
    }

    const actualCalls = readdirSync(hostSourceRoot)
      .filter((filename) => filename.endsWith(".ts") && filename !== "common.ts")
      .reduce((count, filename) => {
        const source = readFileSync(resolve(hostSourceRoot, filename), "utf8");
        return count + (source.match(/\bemitApiError\(/g)?.length ?? 0);
      }, 0);
    expect(actualCalls).toBe(COMMAND_ERROR_MATRIX.length);
  });

  it("keeps description, enrichment, and API-envelope emission owned by common.ts", () => {
    for (const filename of new Set(COMMAND_ERROR_MATRIX.map((row) => row.source))) {
      const source = readFileSync(resolve(hostSourceRoot, filename), "utf8");
      expect(source, `${filename} must not describe API errors locally`).not.toMatch(/\bdescribeApiError\b/);
      expect(source, `${filename} must not retain a local API-error wrapper`).not.toMatch(
        /\b(?:emitControlError|filesError)\b/
      );
    }

    const common = readFileSync(resolve(hostSourceRoot, "common.ts"), "utf8");
    expect(common.match(/describeApiError\(err\)/g)).toHaveLength(1);
    expect(common.match(/export function emitApiError\(/g)).toHaveLength(1);
  });
});
