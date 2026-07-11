/**
 * DX1: `aex login` / `aex logout` / `aex auth status`.
 * - login validates via whoami BEFORE persisting; a bad token is never written.
 * - logout clears the store.
 * - auth status never prints the token value.
 */
import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { type CliIO, type StoredCliConfig } from "../src/internal.js";
import { canonicalWhoami } from "./canonical-whoami.js";

function makeIo(opts: {
  argv: readonly string[];
  stored?: StoredCliConfig | null;
  whoamiStatus?: number;
}): {
  io: CliIO;
  out: () => string;
  err: () => string;
  exit: () => number | null;
  writes: StoredCliConfig[];
  cleared: () => number;
  calls: string[];
} {
  const state = { stdout: "", stderr: "", exit: null as number | null };
  const writes: StoredCliConfig[] = [];
  let cleared = 0;
  let stored = opts.stored ?? null;
  const calls: string[] = [];
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async () => {
      throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
    },
    writeFile: async () => {
      throw new Error("writeFile not configured");
    },
    cwd: () => "/tmp",
    fetchImpl: async (url) => {
      calls.push(String(url));
      const status = opts.whoamiStatus ?? 200;
      if (status !== 200) {
        return new Response(JSON.stringify({ error: "unauthorized" }), {
          status,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response(JSON.stringify(canonicalWhoami("ws-9")), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    },
    stdout: (c) => {
      state.stdout += c;
    },
    stderr: (c) => {
      state.stderr += c;
    },
    exit: (code) => {
      state.exit = code;
    },
    configStore: {
      location: () => "/home/u/.config/aex/config.json",
      read: async () => stored,
      write: async (config) => {
        writes.push(config);
        stored = config;
      },
      clear: async () => {
        cleared++;
        stored = null;
      }
    }
  };
  return {
    io,
    out: () => state.stdout,
    err: () => state.stderr,
    exit: () => state.exit,
    writes,
    cleared: () => cleared,
    calls
  };
}

describe("aex login", () => {
  it("validates via whoami then persists the token + url", async () => {
    const cap = makeIo({
      argv: ["login", "--api-key", "tok-abc", "--aex-url", "https://dev.example"]
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls).toEqual(["https://dev.example/api/whoami"]);
    expect(cap.writes).toHaveLength(1);
    expect(cap.writes[0]).toMatchObject({ schemaVersion: 1, apiKey: "tok-abc", aexUrl: "https://dev.example" });
    const printed = JSON.parse(cap.out().trim()) as { ok: boolean; workspace?: string; configPath: string };
    expect(printed.ok).toBe(true);
    expect(printed.workspace).toBe("ws-9");
    expect(printed.configPath).toBe("/home/u/.config/aex/config.json");
  });

  it("does NOT persist a bad token (whoami fails)", async () => {
    const cap = makeIo({
      argv: ["login", "--api-key", "bad-tok"],
      whoamiStatus: 401
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(1);
    expect(cap.writes).toHaveLength(0);
    const printed = JSON.parse(cap.err().trim()) as { error: string; status?: number; remedy?: string };
    expect(printed.error).toBe("login_failed");
    expect(printed.status).toBe(401);
    expect(printed.remedy).toContain("aex login");
  });

  it("requires a token", async () => {
    const cap = makeIo({ argv: ["login"] });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("usage: aex login");
    expect(cap.calls).toHaveLength(0);
  });
});

describe("aex logout", () => {
  it("clears the store", async () => {
    const cap = makeIo({ argv: ["logout"], stored: { apiKey: "tok" } });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.cleared()).toBe(1);
    const printed = JSON.parse(cap.out().trim()) as { ok: boolean; cleared: boolean };
    expect(printed).toMatchObject({ ok: true, cleared: true });
  });
});

describe("aex auth status", () => {
  it("shows config path + hasToken without printing the token value", async () => {
    const cap = makeIo({
      argv: ["auth", "status"],
      stored: { apiKey: "super-secret-1234", aexUrl: "https://dev.example" }
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    const printed = JSON.parse(cap.out().trim()) as {
      configPath: string;
      hasToken: boolean;
      tokenSuffix?: string;
      aexUrl?: string;
    };
    expect(printed.hasToken).toBe(true);
    expect(printed.aexUrl).toBe("https://dev.example");
    expect(printed.tokenSuffix).toBe("1234");
    expect(cap.out()).not.toContain("super-secret-1234");
  });

  it("reports hasToken=false when nothing is stored", async () => {
    const cap = makeIo({ argv: ["auth", "status"], stored: null });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    const printed = JSON.parse(cap.out().trim()) as { hasToken: boolean };
    expect(printed.hasToken).toBe(false);
  });
});
