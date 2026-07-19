/**
 * Edge cases of the CONTROL-PLANE bearer resolver (`resolveControlPlaneHostFlags`)
 * and its data-plane sibling (`resolveCommonHostFlags`). Existing coverage
 * (control-plane-cmds / auth-resolution) exercises `defaultOrgId` and the
 * account-token happy path through `executeCli`; these unit-test the resolver
 * directly to pin the pieces the command output does NOT surface:
 *   (a) the `source:"workspace"` LAST-RESORT fallback (control-plane verb with
 *       ONLY a stored workspace key and no account token);
 *   (b) `defaultWorkspaceId` propagation onto the resolved result;
 *   (c) the disambiguation when BOTH `accountToken` and `apiKey` are stored —
 *       the control-plane resolver picks `accountToken`, the data-plane resolver
 *       picks `apiKey`.
 */
import { describe, expect, it } from "vitest";
import { resolveCommonHostFlags, resolveControlPlaneHostFlags } from "../src/host/common.js";
import type { CliIO, StoredCliConfig } from "../src/internal.js";

/** Minimal CliIO wired with a fixed stored config; captures stderr. */
function makeIo(stored: StoredCliConfig | null): { io: CliIO; err: () => string } {
  let err = "";
  const io: CliIO = {
    argv: [],
    readFile: async () => {
      throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
    },
    writeFile: async () => {
      throw new Error("writeFile not configured");
    },
    cwd: () => "/tmp",
    fetchImpl: async () => {
      throw new Error("no network in resolver tests");
    },
    stdout: () => {},
    stderr: (c) => {
      err += c;
    },
    exit: () => {},
    configStore: {
      location: () => "/home/u/.config/aex/config.json",
      read: async () => stored,
      write: async () => {},
      clear: async () => {}
    }
  };
  return { io, err: () => err };
}

const WS_KEY = "aex_dev_euw1_wsp1_secret_crc";
const ACCT_TOKEN = "aexu_stored_accttokenacct1234";

describe("resolveControlPlaneHostFlags — workspace last-resort fallback", () => {
  it("falls back to the stored workspace key with source:\"workspace\" when no account token exists", async () => {
    const { io } = makeIo({ schemaVersion: 1, apiKey: WS_KEY });
    const result = await resolveControlPlaneHostFlags(io, []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    // A workspace key is the last resort: it lets the request produce an
    // actionable 401/403 rather than a confusing "no credential" pre-flight.
    expect(result.source).toBe("workspace");
    expect(result.flags.apiKey).toBe(WS_KEY);
    expect(result.flags.aexUrl).toBe("https://api.aex.dev");
  });

  it("names the workspace-key source (never the value) under --debug", async () => {
    const { io, err } = makeIo({ schemaVersion: 1, apiKey: WS_KEY });
    const result = await resolveControlPlaneHostFlags(io, ["--debug"]);
    expect(result.ok).toBe(true);
    expect(err()).toContain("control-plane auth: stored workspace key");
    expect(err()).not.toContain(WS_KEY);
  });

  it("still fails with the actionable login hint when nothing is stored", async () => {
    const { io } = makeIo(null);
    const result = await resolveControlPlaneHostFlags(io, []);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("aex login");
  });
});

describe("resolveControlPlaneHostFlags — defaultWorkspaceId propagation", () => {
  it("threads the stored defaultWorkspaceId onto the resolved result", async () => {
    const { io } = makeIo({ schemaVersion: 1, accountToken: ACCT_TOKEN, defaultWorkspaceId: "wsp_default" });
    const result = await resolveControlPlaneHostFlags(io, []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.source).toBe("account");
    expect(result.defaultWorkspaceId).toBe("wsp_default");
    // Omitted keys stay absent rather than surfacing as `undefined`.
    expect(result.defaultOrgId).toBeUndefined();
  });

  it("threads both defaultOrgId and defaultWorkspaceId when present", async () => {
    const { io } = makeIo({
      schemaVersion: 1,
      accountToken: ACCT_TOKEN,
      defaultOrgId: "org_default",
      defaultWorkspaceId: "wsp_default"
    });
    const result = await resolveControlPlaneHostFlags(io, []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.defaultOrgId).toBe("org_default");
    expect(result.defaultWorkspaceId).toBe("wsp_default");
  });
});

describe("control-plane vs data-plane disambiguation when BOTH tokens are stored", () => {
  const bothStored: StoredCliConfig = { schemaVersion: 1, apiKey: WS_KEY, accountToken: ACCT_TOKEN };

  it("the control-plane resolver picks the ACCOUNT token", async () => {
    const { io } = makeIo(bothStored);
    const result = await resolveControlPlaneHostFlags(io, []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.source).toBe("account");
    expect(result.flags.apiKey).toBe(ACCT_TOKEN);
  });

  it("the data-plane resolver picks the WORKSPACE key", async () => {
    const { io } = makeIo(bothStored);
    const result = await resolveCommonHostFlags(io, []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.flags.apiKey).toBe(WS_KEY);
  });

  it("an explicit --api-key flag wins over both stored tokens on the control plane", async () => {
    const { io } = makeIo(bothStored);
    const result = await resolveControlPlaneHostFlags(io, ["--api-key", "aexu_flag_pattttttttttttttt"]);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.source).toBe("flag");
    expect(result.flags.apiKey).toBe("aexu_flag_pattttttttttttttt");
  });
});
