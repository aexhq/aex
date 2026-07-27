/**
 * Offline construction guards for the `McpServer` primitive, exercised through a
 * clean installed `@aexhq/sdk` (blackbox, child process, cwd = install tempdir).
 *
 * These cases were the "(offline)" half of
 * `test/live/edge-mcp-egress.user.test.ts`: every one of them is a constructor
 * call that never touches the network, yet they rode a live runtime-matrix job
 * and shared a child process with paid SSRF submission probes. Split out
 * 2026-07-27 so they run on every push instead of only on a deploy dispatch.
 *
 * The live file keeps what genuinely needs a plane: the fail-closed submission
 * probes (SSRF/IMDS, RFC1918, server-side name pattern, duplicate names) and the
 * `networking:limited` egress-allowlist session.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { newId } from "@aexhq/contracts";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

interface ConstructionCase {
  readonly label: string;
  readonly threw: boolean;
  readonly message: string;
  readonly extra?: unknown;
}

/**
 * MINTED by the id owner, never hand-written. The value this case inherited from
 * the live suite was a literal that did not satisfy the workspace id shape, so
 * the "good" half of the fromId pair had been asserting a REJECTION all along —
 * invisible while the live lane was red. The installed SDK's curated public
 * surface does not re-export the id helpers, so the id is minted here and
 * interpolated into the child script.
 */
const VALID_MCP_ID = newId("mcp");

const SCRIPT = String.raw`
import { McpServer, MCP_SERVER_NAME_PATTERN } from "@aexhq/sdk";

const construction = [];
function attempt(label, fn) {
  try {
    const value = fn();
    construction.push({ label, threw: false, message: "", extra: value ?? null });
  } catch (err) {
    construction.push({ label, threw: true, message: err && err.message ? String(err.message) : String(err) });
  }
}

attempt("valid-remote-with-headers", () => {
  const m = McpServer.remote({ name: "deepwiki", url: "https://mcp.deepwiki.com/mcp", headers: { Authorization: "Bearer XYZ" } });
  return { sub: m.toSubmissionEntry(), sec: m.toSecretEntry() ?? null };
});
attempt("missing-url", () => McpServer.remote({ name: "noturl" }));
attempt("missing-name", () => McpServer.remote({ url: "https://example.com/mcp" }));
attempt("empty-name", () => McpServer.remote({ name: "", url: "https://example.com/mcp" }));
attempt("stdio-transport", () => new McpServer({ kind: "inline", name: "x", url: "https://x.com/mcp", transport: "stdio" }));
attempt("stdio-command-field", () => new McpServer({ name: "x", url: "https://x.com/mcp", command: "node" }));
attempt("bad-name-uppercase", () => {
  const m = McpServer.remote({ name: "UPPER_CASE", url: "https://example.com/mcp" });
  return { patternMatches: MCP_SERVER_NAME_PATTERN.test("UPPER_CASE"), constructedName: m.name };
});
attempt("fromId-bad", () => McpServer.fromId("not-a-valid-id"));
attempt("fromId-good", () => {
  const m = McpServer.fromId(${JSON.stringify(VALID_MCP_ID)});
  return { kind: m.kind, id: m.id };
});

process.stdout.write(JSON.stringify({ construction }));
`;

describe("installed SDK McpServer construction guards", () => {
  let install: InstallResult;
  let construction: readonly ConstructionCase[];

  beforeAll(async () => {
    install = await installAex();
    const path = join(install.installDir, "mcp-construction.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (child.exitCode !== 0) {
      throw new Error(`mcp-construction.mjs exited ${child.exitCode}\n${child.stderr}`);
    }
    construction = (JSON.parse(child.stdout) as { construction: ConstructionCase[] }).construction;
  }, 300_000);

  afterAll(() => install?.cleanup());

  function ctor(label: string): ConstructionCase {
    const found = construction.find((entry) => entry.label === label);
    if (!found) {
      throw new Error(
        `construction case ${label} missing; got: ${construction.map((entry) => entry.label).join(", ")}`
      );
    }
    return found;
  }

  it("valid McpServer.remote(+headers) constructs; submission entry omits the header", () => {
    const withHeaders = ctor("valid-remote-with-headers");
    expect(withHeaders.threw, withHeaders.message).toBe(false);
    const extra = withHeaders.extra as { sub: Record<string, unknown>; sec: Record<string, unknown> | null };
    // Non-secret wire entry must be {name,url} only — never carry the header.
    expect(Object.keys(extra.sub).sort()).toEqual(["name", "url"]);
    expect(JSON.stringify(extra.sub)).not.toContain("Authorization");
    // Secret entry carries the header out-of-band.
    expect(extra.sec && JSON.stringify(extra.sec)).toContain("Authorization");
  });

  it("missing url / missing name / empty name throw clear construction errors", () => {
    expect(ctor("missing-url").threw).toBe(true);
    expect(ctor("missing-url").message).toMatch(/url is required/i);
    expect(ctor("missing-name").threw).toBe(true);
    expect(ctor("missing-name").message).toMatch(/name is required/i);
    expect(ctor("empty-name").threw).toBe(true);
  });

  it("stdio-shaped MCP inputs are rejected at construction (transport + command field)", () => {
    expect(ctor("stdio-transport").threw).toBe(true);
    expect(ctor("stdio-command-field").threw).toBe(true);
  });

  it("McpServer.fromId validates the workspace id pattern", () => {
    expect(ctor("fromId-bad").threw).toBe(true);
    expect(ctor("fromId-good").threw, ctor("fromId-good").message).toBe(false);
  });

  it("DX note: SDK constructor does NOT pre-validate the MCP name pattern (server enforces it)", () => {
    // Documents the client-side gap: an invalid name constructs fine; the
    // canonical pattern is only enforced server-side, which the live
    // edge-mcp-egress sweep asserts.
    const badName = ctor("bad-name-uppercase");
    expect(badName.threw).toBe(false);
    const extra = badName.extra as { patternMatches: boolean; constructedName: string };
    expect(extra.patternMatches).toBe(false);
    expect(extra.constructedName).toBe("UPPER_CASE");
  });
});
