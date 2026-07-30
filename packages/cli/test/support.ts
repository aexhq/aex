import { createHash } from "node:crypto";
import type { CliIO } from "../src/internal.js";

export interface FetchCall {
  readonly url: string;
  readonly init: RequestInit;
  readonly body: unknown;
}

export interface Harness {
  readonly io: CliIO;
  readonly calls: readonly FetchCall[];
  readonly writes: ReadonlyMap<string, Uint8Array>;
  readonly appends: number;
  readonly stdout: string;
  readonly stdoutBytes: Uint8Array;
  readonly stderr: string;
  readonly exitCode: number | null;
}

export function makeHarness(options: {
  readonly args: readonly string[];
  readonly fetch?: (call: FetchCall) => Response | Promise<Response>;
  readonly textFiles?: Readonly<Record<string, string>>;
  readonly byteFiles?: Readonly<Record<string, Uint8Array>>;
  readonly cwd?: string;
  readonly stdinIsTTY?: boolean;
}): Harness {
  const state = {
    calls: [] as FetchCall[],
    writes: new Map<string, Uint8Array>(),
    appends: 0,
    stdout: "",
    stdoutBytes: new Uint8Array(),
    stderr: "",
    exitCode: null as number | null
  };
  const textFiles = options.textFiles ?? {};
  const byteFiles = options.byteFiles ?? {};
  const cwd = options.cwd ?? "C:\\cli-test";
  const io: CliIO = {
    argv: ["bun", "aex", ...options.args],
    cwd: () => cwd,
    readFile: async (path) => {
      const value = textFiles[path];
      if (value === undefined) throw Object.assign(new Error("missing"), { code: "ENOENT" });
      return value;
    },
    readFileBytes: async (path) => {
      const written = state.writes.get(path);
      if (written) return written;
      const value = byteFiles[path];
      if (value) return value;
      const text = textFiles[path];
      if (text !== undefined) return new TextEncoder().encode(text);
      throw Object.assign(new Error("missing"), { code: "ENOENT" });
    },
    readStdin: async () => textFiles.stdin ?? "",
    stdinIsTTY: options.stdinIsTTY ?? true,
    writeFile: async (path, bytes) => { state.writes.set(path, bytes); },
    appendFile: async (path, bytes) => {
      const prior = state.writes.get(path) ?? byteFiles[path] ?? new Uint8Array();
      const merged = new Uint8Array(prior.byteLength + bytes.byteLength);
      merged.set(prior);
      merged.set(bytes, prior.byteLength);
      state.writes.set(path, merged);
      state.appends += 1;
    },
    fileSize: async (path) => {
      const written = state.writes.get(path);
      if (written) return written.byteLength;
      const bytes = byteFiles[path];
      if (bytes) return bytes.byteLength;
      const text = textFiles[path];
      if (text !== undefined) return new TextEncoder().encode(text).byteLength;
      throw Object.assign(new Error("missing"), { code: "ENOENT" });
    },
    sha256File: async (path) => {
      const written = state.writes.get(path) ?? byteFiles[path];
      if (!written) throw Object.assign(new Error("missing"), { code: "ENOENT" });
      return `sha256:${createHash("sha256").update(written).digest("hex")}`;
    },
    renameFile: async (from, to) => {
      const value = state.writes.get(from);
      if (!value) throw new Error(`missing partial ${from}`);
      state.writes.delete(from);
      state.writes.set(to, value);
    },
    fileExists: async (path) => state.writes.has(path) || path in byteFiles || path in textFiles,
    fetchImpl: async (input, init = {}) => {
      let body: unknown;
      if (typeof init.body === "string") {
        try { body = JSON.parse(init.body); } catch { body = init.body; }
      }
      const call = { url: String(input), init, body };
      state.calls.push(call);
      return options.fetch?.(call) ?? json({});
    },
    stdout: (chunk) => { state.stdout += chunk; },
    stdoutBytes: (chunk) => {
      const merged = new Uint8Array(state.stdoutBytes.byteLength + chunk.byteLength);
      merged.set(state.stdoutBytes);
      merged.set(chunk, state.stdoutBytes.byteLength);
      state.stdoutBytes = merged;
    },
    stderr: (chunk) => { state.stderr += chunk; },
    exit: (code) => { state.exitCode = code; }
  };
  return {
    io,
    get calls() { return state.calls; },
    get writes() { return state.writes; },
    get appends() { return state.appends; },
    get stdout() { return state.stdout; },
    get stdoutBytes() { return state.stdoutBytes; },
    get stderr() { return state.stderr; },
    get exitCode() { return state.exitCode; }
  };
}

export function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });
}
