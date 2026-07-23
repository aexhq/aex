import fc from "fast-check";
import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import { AEX_INDEX_PATH, type CliIO, type StoredCliConfig } from "../src/internal.js";
import { prepareHostCommand } from "../src/host/common.js";

const DATA_VERBS = [
  "start", "status", "deliveries", "wait", "events", "tail", "inspect", "files",
  "download", "cancel", "delete", "delete-asset", "sessions", "whoami", "billing", "webhooks"
] as const;
const CONTROL_VERBS = ["orgs", "workspaces", "keys"] as const;
const AUTHENTICATED_VERBS = [...DATA_VERBS, ...CONTROL_VERBS] as const;

function makeIo(options: {
  readonly argv?: readonly string[];
  readonly stored?: StoredCliConfig | null;
  readonly manifest?: "absent" | "present" | "unreadable";
} = {}): {
  readonly io: CliIO;
  readonly stderr: () => string;
  readonly exit: () => number | null;
  readonly configReads: () => number;
  readonly fetches: () => number;
} {
  let stderr = "";
  let exit: number | null = null;
  let configReads = 0;
  let fetches = 0;
  const manifest = options.manifest ?? "absent";
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...(options.argv ?? [])],
    readFile: async (path) => {
      expect(path).toBe(AEX_INDEX_PATH);
      if (manifest === "present") return "{}";
      if (manifest === "unreadable") throw Object.assign(new Error("permission denied"), { code: "EACCES" });
      throw Object.assign(new Error("not found"), { code: "ENOENT" });
    },
    writeFile: async () => {
      throw new Error("writeFile not configured");
    },
    cwd: () => "/tmp",
    fetchImpl: async () => {
      fetches++;
      throw new Error("network must not be reached");
    },
    stdout: () => {},
    stderr: (chunk) => { stderr += chunk; },
    exit: (code) => { exit = code; },
    configStore: {
      location: () => "/home/u/.config/aex/config.json",
      read: async () => {
        configReads++;
        return options.stored ?? null;
      },
      write: async () => {},
      clear: async () => {}
    }
  };
  return {
    io,
    stderr: () => stderr,
    exit: () => exit,
    configReads: () => configReads,
    fetches: () => fetches
  };
}

describe("typed authenticated host-command preparation", () => {
  it("returns policy-discriminated data and control results without erasing control metadata", async () => {
    const stored: StoredCliConfig = {
      schemaVersion: 1,
      apiKey: "workspace-token",
      accountToken: "account-token",
      aexUrl: "https://stored.example",
      defaultOrgId: "org-default",
      defaultWorkspaceId: "workspace-default"
    };
    const data = await prepareHostCommand(makeIo({ stored }).io, ["position"], { verb: "status", auth: "data" });
    expect(data).toEqual({
      ok: true,
      auth: "data",
      flags: { apiKey: "workspace-token", aexUrl: "https://stored.example", debug: false, json: false },
      rest: ["position"]
    });

    const control = await prepareHostCommand(makeIo({ stored }).io, ["list"], { verb: "orgs", auth: "control" });
    expect(control).toEqual({
      ok: true,
      auth: "control",
      flags: { apiKey: "account-token", aexUrl: "https://stored.example", debug: false, json: false },
      rest: ["list"],
      source: "account",
      defaultOrgId: "org-default",
      defaultWorkspaceId: "workspace-default"
    });
  });

  it("preserves data/control flag, stored, URL, source, debug, and negative precedence", async () => {
    const stored: StoredCliConfig = {
      schemaVersion: 1,
      apiKey: "workspace-secret",
      accountToken: "account-secret",
      aexUrl: "https://stored.example"
    };
    const dataCap = makeIo({ stored });
    const data = await prepareHostCommand(
      dataCap.io,
      ["--api-key=flag-secret", "--aex-url=https://flag.example", "--debug", "--json", "arg"],
      { verb: "status", auth: "data" }
    );
    expect(data.ok && data).toMatchObject({
      auth: "data",
      flags: { apiKey: "flag-secret", aexUrl: "https://flag.example", debug: true, json: true },
      rest: ["arg"]
    });
    expect(dataCap.configReads()).toBe(0);
    expect(dataCap.stderr()).toBe("[aex] auth: --api-key flag; aex-url=https://flag.example\n");
    expect(dataCap.stderr()).not.toContain("secret");

    const flaggedDataWithoutUrl = makeIo({ stored });
    const dataDefaultUrl = await prepareHostCommand(flaggedDataWithoutUrl.io, ["--api-key", "flag-secret"], {
      verb: "status",
      auth: "data"
    });
    expect(dataDefaultUrl.ok && dataDefaultUrl.flags.aexUrl).toBe("https://api.aex.dev");
    expect(flaggedDataWithoutUrl.configReads()).toBe(0);

    const controlCap = makeIo({ stored });
    const control = await prepareHostCommand(controlCap.io, ["--api-key", "flag-secret", "--debug"], {
      verb: "keys",
      auth: "control"
    });
    expect(control.ok && control).toMatchObject({
      auth: "control",
      source: "flag",
      flags: { apiKey: "flag-secret", aexUrl: "https://stored.example", debug: true }
    });
    expect(controlCap.configReads()).toBe(1);
    expect(controlCap.stderr()).toBe(
      "[aex] control-plane auth: --api-key flag; aex-url=https://stored.example\n"
    );
    expect(controlCap.stderr()).not.toContain("secret");

    const missingData = makeIo();
    const dataFailure = await prepareHostCommand(missingData.io, [], { verb: "whoami", auth: "data" });
    expect(dataFailure).toEqual({ ok: false, exit: { code: 2 } });
    expect(missingData.stderr()).toBe("no API key — pass --api-key or run `aex login`\n");

    const missingControl = makeIo();
    const controlFailure = await prepareHostCommand(missingControl.io, [], { verb: "orgs", auth: "control" });
    expect(controlFailure).toEqual({ ok: false, exit: { code: 2 } });
    expect(missingControl.stderr()).toBe(
      "no account credential — run `aex login` (device flow) or pass an account PAT via --api-key\n"
    );
  });

  it("preserves extractor failures and allows the start command to own its visible diagnostic provenance", async () => {
    const missingValue = makeIo();
    const failure = await prepareHostCommand(missingValue.io, ["--api-key"], {
      verb: "start",
      auth: "data",
      formatResolutionError: (reason) => `aex start --api-key: ${reason}`
    });
    expect(failure).toEqual({ ok: false, exit: { code: 2 } });
    expect(missingValue.stderr()).toBe("aex start --api-key: --api-key requires a value\n");
    expect(missingValue.configReads()).toBe(0);
  });

  for (const manifest of ["present", "unreadable"] as const) {
    it(`refuses all 19 authenticated verbs before config/network when the manifest is ${manifest}`, async () => {
      for (const verb of AUTHENTICATED_VERBS) {
        const cap = makeIo({ argv: [verb], stored: { apiKey: "workspace", accountToken: "account" }, manifest });
        await executeCli(cap.io);
        expect(cap.exit(), verb).toBe(2);
        expect(cap.stderr(), verb).toContain(`\`aex ${verb}\``);
        expect(cap.stderr(), verb).toContain(
          manifest === "present" ? "cannot execute inside a managed session container" : "Refusing to proceed."
        );
        expect(cap.configReads(), verb).toBe(0);
        expect(cap.fetches(), verb).toBe(0);
      }
    });
  }

  it("refusal wins over malformed argv without producing an extractor diagnostic", async () => {
    const cap = makeIo({ manifest: "present" });
    const result = await prepareHostCommand(cap.io, ["--api-key"], { verb: "status", auth: "data" });
    expect(result).toEqual({ ok: false, exit: { code: 2 } });
    expect(cap.stderr()).toBe(
      "`aex status` is a host command and cannot execute inside a managed session container.\n" +
      "Make HTTP calls from your code and pass credentials through secrets.\n"
    );
    expect(cap.configReads()).toBe(0);
  });

  it("rejects the exhaustive 19-verb matrix without credentials before command parsing or network", async () => {
    for (const verb of AUTHENTICATED_VERBS) {
      const cap = makeIo({ argv: [verb] });
      await executeCli(cap.io);
      expect(cap.exit(), verb).toBe(2);
      expect(cap.stderr(), verb).toBe(
        verb === "start"
          ? "aex start --api-key: no API key — pass --api-key or run `aex login`\n"
          : CONTROL_VERBS.includes(verb as typeof CONTROL_VERBS[number])
            ? "no account credential — run `aex login` (device flow) or pass an account PAT via --api-key\n"
            : "no API key — pass --api-key or run `aex login`\n"
      );
      expect(cap.fetches(), verb).toBe(0);
    }
  });

  it("property: explicit non-empty credentials and URLs dominate arbitrary stored values without network", async () => {
    await fc.assert(fc.asyncProperty(
      fc.string({ minLength: 1 }).filter((value) => !value.includes("\n")),
      fc.webUrl(),
      fc.string({ minLength: 1 }),
      fc.string({ minLength: 1 }),
      async (flagToken, flagUrl, storedWorkspace, storedAccount) => {
        const stored = { apiKey: storedWorkspace, accountToken: storedAccount, aexUrl: "https://stored.example" };
        for (const auth of ["data", "control"] as const) {
          const cap = makeIo({ stored });
          const result = await prepareHostCommand(
            cap.io,
            [`--api-key=${flagToken}`, `--aex-url=${flagUrl}`, "positional"],
            { verb: auth === "data" ? "status" : "orgs", auth }
          );
          expect(result.ok).toBe(true);
          if (!result.ok) continue;
          expect(result.auth).toBe(auth);
          expect(result.flags.apiKey).toBe(flagToken);
          expect(result.flags.aexUrl).toBe(flagUrl);
          expect(result.rest).toEqual(["positional"]);
          expect(cap.fetches()).toBe(0);
        }
      }
    ), { numRuns: 50 });
  });
});
