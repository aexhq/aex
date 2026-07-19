/**
 * WS6 control-plane CLI verbs: `aex orgs` / `aex workspaces` / `aex keys`.
 * These reach the account (control-plane) principal — the bearer resolves from
 * an explicit `--api-key` OR a stored `accountToken` (device-flow login), never
 * a key derived from a self-describing workspace token.
 */
import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import type { CliIO, StoredCliConfig } from "../src/internal.js";

interface Cap {
  io: CliIO;
  out: () => string;
  err: () => string;
  exit: () => number | null;
  calls: Array<{ url: string; method: string; body: unknown; authorization: string | undefined }>;
}

function makeIo(opts: {
  argv: readonly string[];
  stored?: StoredCliConfig | null;
  responder?: (url: string, method: string) => { status?: number; body: unknown };
}): Cap {
  const state = { stdout: "", stderr: "", exit: null as number | null };
  const calls: Cap["calls"] = [];
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
      const method = (init?.method ?? "GET").toUpperCase();
      const headers = init?.headers as Record<string, string> | undefined;
      const authorization = headers
        ? Object.entries(headers).find(([k]) => k.toLowerCase() === "authorization")?.[1]
        : undefined;
      let body: unknown;
      if (typeof init?.body === "string") {
        try {
          body = JSON.parse(init.body) as unknown;
        } catch {
          body = init.body;
        }
      }
      calls.push({ url: String(url), method, body, authorization });
      const res = opts.responder?.(String(url), method) ?? { body: {} };
      return new Response(JSON.stringify(res.body), {
        status: res.status ?? 200,
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
      read: async () => opts.stored ?? null,
      write: async () => {},
      clear: async () => {}
    }
  };
  return { io, out: () => state.stdout, err: () => state.stderr, exit: () => state.exit, calls };
}

const ACCOUNT_STORED: StoredCliConfig = { schemaVersion: 1, accountToken: "aexu_stored_accttokenacct1234" };

describe("aex orgs", () => {
  it("lists orgs using the stored account token as bearer", async () => {
    const cap = makeIo({
      argv: ["orgs"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { orgs: [{ id: "org_1", name: "Acme", role: "admin" }] } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls).toHaveLength(1);
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/orgs");
    expect(cap.calls[0]!.method).toBe("GET");
    expect(cap.calls[0]!.authorization).toBe("Bearer aexu_stored_accttokenacct1234");
    const printed = JSON.parse(cap.out().trim()) as Array<{ id: string }>;
    expect(printed[0]!.id).toBe("org_1");
  });

  it("creates an org via POST /api/orgs with the name in the body", async () => {
    const cap = makeIo({
      argv: ["orgs", "create", "--name", "Acme", "--api-key", "aexu_flag_pattttttttttttttt"],
      responder: () => ({ body: { org: { id: "org_new", name: "Acme" } } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.method).toBe("POST");
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/orgs");
    expect(cap.calls[0]!.body).toEqual({ name: "Acme" });
    expect(cap.calls[0]!.authorization).toBe("Bearer aexu_flag_pattttttttttttttt");
  });

  it("lists org members", async () => {
    const cap = makeIo({
      argv: ["orgs", "members", "org_1"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { members: [{ appUserId: "u1", role: "admin", status: "active" }] } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/orgs/org_1/members");
  });

  it("invites a member with email + role", async () => {
    const cap = makeIo({
      argv: ["orgs", "invite", "org_1", "--email", "a@b.test", "--role", "member"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { invite: { id: "inv_1", orgId: "org_1", email: "a@b.test", role: "member" } } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/orgs/org_1/invites");
    expect(cap.calls[0]!.body).toEqual({ email: "a@b.test", role: "member" });
  });

  it("rejects an invalid --role", async () => {
    const cap = makeIo({
      argv: ["orgs", "invite", "org_1", "--email", "a@b.test", "--role", "superuser"],
      stored: ACCOUNT_STORED
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("--role must be admin or member");
  });

  it("fails with an actionable message when no account credential is present", async () => {
    const cap = makeIo({ argv: ["orgs"], stored: null });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("aex login");
    expect(cap.calls).toHaveLength(0);
  });
});

describe("aex workspaces", () => {
  it("creates a workspace and reveals the one-time key", async () => {
    const cap = makeIo({
      argv: ["workspaces", "create", "--org", "org_1", "--name", "prod"],
      stored: ACCOUNT_STORED,
      responder: () => ({
        body: { workspace: { workspaceId: "wsp_abc", apiKey: "aex_dev_euw1_abc_secret_crc", orgId: "org_1" } }
      })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.method).toBe("POST");
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/workspaces");
    expect(cap.calls[0]!.body).toEqual({ orgId: "org_1", name: "prod" });
    // The CLI prints the one-time key verbatim so the operator can capture it.
    const printed = JSON.parse(cap.out().trim()) as { workspaceId: string; apiKey: string };
    expect(printed.workspaceId).toBe("wsp_abc");
    expect(printed.apiKey).toBe("aex_dev_euw1_abc_secret_crc");
  });

  it("uses the stored defaultOrgId when --org is omitted", async () => {
    const cap = makeIo({
      argv: ["workspaces", "create", "--name", "prod"],
      stored: { ...ACCOUNT_STORED, defaultOrgId: "org_default" },
      responder: () => ({ body: { workspace: { workspaceId: "wsp_x", apiKey: "aex_dev_euw1_x_s_c" } } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.body).toEqual({ orgId: "org_default", name: "prod" });
  });

  it("errors when no org is available", async () => {
    const cap = makeIo({ argv: ["workspaces", "create", "--name", "prod"], stored: ACCOUNT_STORED });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("no org");
    expect(cap.calls).toHaveLength(0);
  });

  it("lists and deletes workspaces", async () => {
    const listCap = makeIo({
      argv: ["workspaces"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { workspaces: [{ id: "wsp_1", name: "w", orgId: "org_1" }] } })
    });
    await executeCli(listCap.io);
    expect(listCap.exit()).toBe(0);
    expect(listCap.calls[0]!.url).toBe("https://api.aex.dev/api/workspaces");

    const delCap = makeIo({
      argv: ["workspaces", "delete", "wsp_1"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: {} })
    });
    await executeCli(delCap.io);
    expect(delCap.exit()).toBe(0);
    expect(delCap.calls[0]!.method).toBe("DELETE");
    expect(delCap.calls[0]!.url).toBe("https://api.aex.dev/api/workspaces/wsp_1");
  });
});

describe("aex keys", () => {
  it("mints a workspace key from a positional workspace id", async () => {
    const cap = makeIo({
      argv: ["keys", "create", "wsp_1", "--name", "ci"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { key: { id: "key_1", apiKey: "aex_dev_euw1_1_s_c", workspaceId: "wsp_1" } } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.method).toBe("POST");
    expect(cap.calls[0]!.url).toBe("https://api.aex.dev/api/keys");
    expect(cap.calls[0]!.body).toEqual({ workspaceId: "wsp_1", name: "ci" });
    const printed = JSON.parse(cap.out().trim()) as { id: string; apiKey: string };
    expect(printed.apiKey).toBe("aex_dev_euw1_1_s_c");
  });

  it("mints an account PAT with --account", async () => {
    const cap = makeIo({
      argv: ["keys", "create", "--account", "--name", "headless"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { key: { id: "key_pat", apiKey: "aexu_newpat_tokentokentoken", kind: "account" } } })
    });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(0);
    expect(cap.calls[0]!.body).toEqual({ account: true, name: "headless" });
  });

  it("rejects passing both a workspace id and --account", async () => {
    const cap = makeIo({ argv: ["keys", "create", "wsp_1", "--account"], stored: ACCOUNT_STORED });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("not both");
    expect(cap.calls).toHaveLength(0);
  });

  it("lists and deletes keys", async () => {
    const listCap = makeIo({
      argv: ["keys"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: { keys: [{ id: "key_1", kind: "workspace" }] } })
    });
    await executeCli(listCap.io);
    expect(listCap.exit()).toBe(0);
    expect(listCap.calls[0]!.url).toBe("https://api.aex.dev/api/keys");

    const delCap = makeIo({
      argv: ["keys", "delete", "key_1"],
      stored: ACCOUNT_STORED,
      responder: () => ({ body: {} })
    });
    await executeCli(delCap.io);
    expect(delCap.exit()).toBe(0);
    expect(delCap.calls[0]!.method).toBe("DELETE");
    expect(delCap.calls[0]!.url).toBe("https://api.aex.dev/api/keys/key_1");
  });
});
