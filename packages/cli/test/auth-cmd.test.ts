/**
 * DX1: `aex login` / `aex logout` / `aex auth status`.
 * - login validates via whoami BEFORE persisting; a bad token is never written.
 * - logout clears the store.
 * - auth status never prints the token value.
 */
import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import { type CliIO, type StoredCliConfig } from "../src/internal.js";
import { canonicalWhoami } from "./canonical-whoami.js";

function makeIo(opts: {
  argv: readonly string[];
  stored?: StoredCliConfig | null;
  whoamiStatus?: number;
  fetchHandler?: (url: string, init?: RequestInit) => Promise<Response>;
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
    fetchImpl: async (url, init) => {
      calls.push(String(url));
      if (opts.fetchHandler) return opts.fetchHandler(String(url), init);
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

  it("workspace key (aex_…) validates via the DATA-plane whoami and persists apiKey", async () => {
    // The default stub answers /api/whoami with an api_key principal.
    const cap = makeIo({
      argv: ["login", "--api-key", "aex_dev_euw1_wsp1_secret_crc", "--aex-url", "https://dev.example"]
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls).toEqual(["https://dev.example/api/whoami"]);
    expect(cap.writes).toHaveLength(1);
    expect(cap.writes[0]).toMatchObject({
      schemaVersion: 1,
      apiKey: "aex_dev_euw1_wsp1_secret_crc",
      aexUrl: "https://dev.example"
    });
    // A workspace key is NEVER stored as the account credential.
    expect(cap.writes[0]!.accountToken).toBeUndefined();
  });

  it("account PAT (aexu_…) validates via the CONTROL-plane whoami and persists accountToken", async () => {
    const cap = makeIo({
      argv: ["login", "--api-key", "aexu_pat_tokentokentokentoken", "--aex-url", "https://ctrl.example"],
      // Control-plane whoami answers with an account_token principal (not api_key).
      fetchHandler: async () =>
        new Response(
          JSON.stringify({
            ok: true,
            principalType: "account_token",
            appUserId: "usr_1",
            orgId: "org_1",
            tokenId: "atk_1",
            tokenName: "cli",
            tokenKind: "account",
            scopes: []
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        )
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls).toEqual(["https://ctrl.example/api/whoami"]);
    expect(cap.writes).toHaveLength(1);
    expect(cap.writes[0]).toMatchObject({
      schemaVersion: 1,
      accountToken: "aexu_pat_tokentokentokentoken",
      aexUrl: "https://ctrl.example"
    });
    // A PAT is NEVER stored as the data-plane workspace key.
    expect(cap.writes[0]!.apiKey).toBeUndefined();
    const printed = JSON.parse(cap.out().trim()) as { ok: boolean; accountAuthorized?: boolean };
    expect(printed.ok).toBe(true);
    expect(printed.accountAuthorized).toBe(true);
    // The PAT value is never printed on stdout.
    expect(cap.out()).not.toContain("aexu_pat_tokentokentokentoken");
  });

  it("does NOT persist a bad account PAT (control-plane whoami fails)", async () => {
    const cap = makeIo({
      argv: ["login", "--api-key", "aexu_bad_tokentokentokentoken", "--aex-url", "https://ctrl.example"],
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

});

describe("aex login (device flow)", () => {
  function deviceHandler(steps: {
    code: Record<string, unknown>;
    tokens: ReadonlyArray<{ status?: number; body: Record<string, unknown> }>;
  }): (url: string) => Promise<Response> {
    let tokenCall = 0;
    return async (url: string) => {
      if (url.endsWith("/api/device/code")) {
        return new Response(JSON.stringify(steps.code), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith("/api/device/token")) {
        const step = steps.tokens[Math.min(tokenCall, steps.tokens.length - 1)]!;
        tokenCall += 1;
        return new Response(JSON.stringify(step.body), {
          status: step.status ?? 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response("{}", { status: 404, headers: { "content-type": "application/json" } });
    };
  }

  it("bare `aex login` runs the device flow and persists an account token", async () => {
    const cap = makeIo({
      argv: ["login", "--aex-url", "https://dev.example"],
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-1",
          user_code: "WXYZ-1234",
          verification_uri: "https://dev.example/device",
          interval: 0,
          expires_in: 300
        },
        tokens: [{ body: { access_token: "aexu_accttokenaccttokenacct" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    // /device/code, then /device/token
    expect(cap.calls).toEqual([
      "https://dev.example/api/device/code",
      "https://dev.example/api/device/token"
    ]);
    // Verification instructions go to stderr; the account token is never printed.
    expect(cap.err()).toContain("WXYZ-1234");
    expect(cap.err()).toContain("https://dev.example/device");
    expect(cap.err()).not.toContain("aexu_accttokenaccttokenacct");
    expect(cap.out()).not.toContain("aexu_accttokenaccttokenacct");
    expect(cap.writes).toHaveLength(1);
    expect(cap.writes[0]).toMatchObject({
      schemaVersion: 1,
      accountToken: "aexu_accttokenaccttokenacct",
      aexUrl: "https://dev.example"
    });
    const printed = JSON.parse(cap.out().trim()) as { ok: boolean; accountAuthorized?: boolean };
    expect(printed.ok).toBe(true);
    expect(printed.accountAuthorized).toBe(true);
  });

  it("keeps polling on authorization_pending then persists on approval", async () => {
    const cap = makeIo({
      argv: ["login"],
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-2",
          user_code: "AAAA-0000",
          verification_uri: "https://api.aex.dev/device",
          interval: 0,
          expires_in: 300
        },
        tokens: [
          { status: 400, body: { error: "authorization_pending" } },
          { body: { access_token: "aexu_secondtry_tokentokentoken" } }
        ]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    // Two token polls (pending, then success).
    expect(cap.calls.filter((u) => u.endsWith("/api/device/token"))).toHaveLength(2);
    expect(cap.writes[0]).toMatchObject({ accountToken: "aexu_secondtry_tokentokentoken" });
  });

  it("does NOT persist when authorization is denied", async () => {
    const cap = makeIo({
      argv: ["login"],
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-3",
          user_code: "BBBB-1111",
          verification_uri: "https://api.aex.dev/device",
          interval: 0,
          expires_in: 300
        },
        tokens: [{ status: 400, body: { error: "access_denied" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(1);
    expect(cap.writes).toHaveLength(0);
    // Human instructions and the JSON error both go to stderr; the error is the
    // last (JSON) line.
    const lastLine = cap.err().trim().split("\n").at(-1)!;
    const printed = JSON.parse(lastLine) as { error: string };
    expect(printed.error).toBe("login_denied");
  });

  it("preserves an existing workspace key when adding an account token", async () => {
    const cap = makeIo({
      argv: ["login"],
      stored: { schemaVersion: 1, apiKey: "ws-key-existing" },
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-4",
          user_code: "CCCC-2222",
          verification_uri: "https://api.aex.dev/device",
          interval: 0,
          expires_in: 300
        },
        tokens: [{ body: { access_token: "aexu_coexist_tokentokentoken" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.writes[0]).toMatchObject({
      apiKey: "ws-key-existing",
      accountToken: "aexu_coexist_tokentokentoken"
    });
  });

  it("reports login_expired when the device code expires before approval", async () => {
    const cap = makeIo({
      argv: ["login"],
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-exp",
          user_code: "DDDD-3333",
          verification_uri: "https://api.aex.dev/device",
          interval: 0,
          expires_in: 300
        },
        tokens: [{ status: 400, body: { error: "expired_token" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(1);
    expect(cap.writes).toHaveLength(0);
    const lastLine = cap.err().trim().split("\n").at(-1)!;
    const printed = JSON.parse(lastLine) as { error: string; message: string };
    expect(printed.error).toBe("login_expired");
    expect(printed.message).toContain("aex login");
  });

  it("reports login_timeout when the approval deadline elapses while still pending", async () => {
    // expires_in:0 → the deadline is ~1s out; a single authorization_pending poll
    // + the minimum 1s pacing sleep crosses it, so the loop exits into the
    // timeout branch WITHOUT ever seeing a token.
    const cap = makeIo({
      argv: ["login"],
      fetchHandler: deviceHandler({
        code: {
          device_code: "dc-timeout",
          user_code: "EEEE-4444",
          verification_uri: "https://api.aex.dev/device",
          interval: 0,
          expires_in: 0
        },
        tokens: [{ status: 400, body: { error: "authorization_pending" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(1);
    expect(cap.writes).toHaveLength(0);
    const lastLine = cap.err().trim().split("\n").at(-1)!;
    const printed = JSON.parse(lastLine) as { error: string; message: string };
    expect(printed.error).toBe("login_timeout");
    expect(printed.message).toContain("timed out");
  });

  it("reports device_code_failed on a malformed /api/device/code response", async () => {
    // The device-code response is missing `user_code` — the bootstrap refuses to
    // proceed (and never polls /api/device/token) rather than print a blank code.
    const cap = makeIo({
      argv: ["login"],
      fetchHandler: deviceHandler({
        code: { device_code: "dc-malformed", verification_uri: "https://api.aex.dev/device" },
        tokens: [{ body: { access_token: "aexu_never_reached_tokentoken" } }]
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(1);
    expect(cap.writes).toHaveLength(0);
    expect(cap.calls.filter((u) => u.endsWith("/api/device/token"))).toHaveLength(0);
    const lastLine = cap.err().trim().split("\n").at(-1)!;
    const printed = JSON.parse(lastLine) as { error: string; message: string };
    expect(printed.error).toBe("device_code_failed");
    expect(printed.message).toContain("user_code");
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

  it("reflects a stored account credential (accountToken only) without printing it", async () => {
    const cap = makeIo({
      argv: ["auth", "status"],
      stored: { schemaVersion: 1, accountToken: "aexu_acct_secret_TAIL", aexUrl: "https://ctrl.example" }
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    const printed = JSON.parse(cap.out().trim()) as {
      hasToken: boolean;
      tokenSuffix?: string;
      credential?: string;
      aexUrl?: string;
    };
    expect(printed.hasToken).toBe(true);
    expect(printed.tokenSuffix).toBe("TAIL");
    expect(printed.credential).toBe("account");
    expect(printed.aexUrl).toBe("https://ctrl.example");
    expect(cap.out()).not.toContain("aexu_acct_secret_TAIL");
  });
});
