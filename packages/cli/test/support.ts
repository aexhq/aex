import type { CliIO } from "../src/internal.js";

export interface FetchCall {
  url: string;
  init: RequestInit;
  body: unknown;
}

export interface Cap {
  io: CliIO;
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number | null;
  readonly calls: FetchCall[];
}

/**
 * Minimal DI IO harness for CLI verb tests: captures stdout/stderr/exit,
 * records every fetch call, and serves file reads from an in-memory map.
 */
export function makeIo(opts: {
  argv: readonly string[];
  fetchHandler?: (call: FetchCall) => Response;
  files?: Record<string, string>;
  binaryFiles?: Record<string, Uint8Array>;
  writes?: Map<string, Uint8Array>;
  cwd?: string;
}): Cap {
  const state = { stdout: "", stderr: "", exitCode: null as number | null, calls: [] as FetchCall[] };
  const files = opts.files ?? {};
  const binaryFiles = opts.binaryFiles ?? {};
  const writes = opts.writes ?? new Map<string, Uint8Array>();
  const cwd = opts.cwd ?? "/tmp/cli-test";

  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async (path) => {
      if (!(path in files)) {
        throw Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
      }
      return files[path]!;
    },
    readFileBytes: async (path) => {
      if (path in binaryFiles) return binaryFiles[path]!;
      if (path in files) return new TextEncoder().encode(files[path]!);
      throw Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
    },
    writeFile: async (path, data) => {
      writes.set(path, data);
    },
    cwd: () => cwd,
    fetchImpl: (async (input, init) => {
      const reqInit: RequestInit = init ?? {};
      let body: unknown;
      if (typeof reqInit.body === "string") {
        try {
          body = JSON.parse(reqInit.body);
        } catch {
          body = reqInit.body;
        }
      }
      const url = String(input);
      const call: FetchCall = { url, init: reqInit, body };
      state.calls.push(call);
      const handler = opts.fetchHandler ?? (() => new Response("{}", { status: 200, headers: { "content-type": "application/json" } }));
      return handler(call);
    }) as typeof fetch,
    stdout: (chunk) => { state.stdout += chunk; },
    stderr: (chunk) => { state.stderr += chunk; },
    exit: (code) => { state.exitCode = code; }
  };

  return {
    io,
    get stdout() { return state.stdout; },
    get stderr() { return state.stderr; },
    get exitCode() { return state.exitCode; },
    get calls() { return state.calls; }
  };
}
