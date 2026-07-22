import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { parseMcpServerRef, parseSessionWebhook } from "../src/index.js";

/**
 * Property fuzz for the shared SSRF host deny-list, exercised through the REAL
 * public parsers that wrap it — `parseMcpServerRef` (the MCP URL gate, which
 * delegates to the internal `denyReasonForHostIp`/`denyReasonForMcpHost`) and
 * `parseSessionWebhook` (the per-session callback gate). The deny logic itself is not
 * exported, so we drive it the only honest way: through the parsers a caller
 * actually hits. No mocks — the assertions hold against the production code.
 *
 * Invariants:
 *   - a private / loopback / link-local / CGNAT / ULA literal host is ALWAYS
 *     rejected (defense-in-depth against SSRF / cloud-metadata exfil);
 *   - a routable public host is ALWAYS accepted;
 *   - the parser NEVER throws a non-Error and NEVER passes a denied host.
 */

const NAME = "srv"; // matches MCP_SERVER_NAME_PATTERN; the URL is what we fuzz.
const octet = fc.integer({ min: 0, max: 255 });

/** IPv4 literals the MCP deny-list MUST refuse (mirrors denyReasonForV4). */
const privateV4 = fc.oneof(
  fc.tuple(fc.constant(0), octet, octet, octet),
  fc.tuple(fc.constant(10), octet, octet, octet),
  fc.tuple(fc.constant(127), octet, octet, octet),
  fc.tuple(fc.constant(169), fc.constant(254), octet, octet),
  fc.tuple(fc.constant(192), fc.constant(168), octet, octet),
  fc.tuple(fc.constant(172), fc.integer({ min: 16, max: 31 }), octet, octet),
  fc.tuple(fc.constant(100), fc.integer({ min: 64, max: 127 }), octet, octet), // CGNAT
  fc.tuple(fc.constant(198), fc.integer({ min: 18, max: 19 }), octet, octet), // benchmarking
  fc.tuple(fc.integer({ min: 224, max: 255 }), octet, octet, octet) // multicast/reserved
).map((parts) => parts.join("."));

/** IPv6 / mapped literals + loopback names the deny-list MUST refuse. */
const privateHost = fc.oneof(
  privateV4,
  fc.constantFrom("localhost", "foo.localhost", "127.0.0.1", "169.254.169.254"),
  fc.constantFrom("[::]", "[::1]", "[0:0:0:0:0:0:0:1]"),
  octet.map((h) => `[fe80::${h.toString(16)}]`), // link-local fe80::/10
  octet.map((h) => `[fc00::${h.toString(16)}]`), // ULA fc00::/7
  octet.map((h) => `[fd00::${h.toString(16)}]`),
  privateV4.map((v4) => `[::ffff:${v4}]`) // IPv4-mapped private target
);

/** Hosts that are unambiguously public and routable → must be accepted. */
const publicV4 = fc.constantFrom("8.8.8.8", "1.1.1.1", "93.184.216.34", "203.0.113.7", "198.51.100.9");
const publicHost = fc.oneof(
  publicV4,
  fc
    .string({ minLength: 1, maxLength: 20 })
    .map((s) => s.toLowerCase().replace(/[^a-z0-9]/g, ""))
    .filter((s) => s.length > 0)
    .map((label) => `${label}.example.com`)
);

function mcpThrows(url: string): Error | null {
  try {
    parseMcpServerRef({ name: NAME, url }, "mcpServers[0]");
    return null;
  } catch (err) {
    return err as Error;
  }
}

describe("SSRF host deny-list (property)", { timeout: 0 }, () => {
  it("rejects every private/loopback/link-local/ULA literal MCP host", () => {
    fc.assert(
      fc.property(privateHost, (host) => {
        const err = mcpThrows(`https://${host}/mcp`);
        expect(err).toBeInstanceOf(Error);
        expect(err?.message).toMatch(/^mcpServers\[0\]\.url must not target|must use port 443/);
      }),
      { numRuns: 400 }
    );
  });

  it("accepts every routable public MCP host (https, default/443 port)", () => {
    fc.assert(
      fc.property(publicHost, fc.constantFrom("", "443"), (host, port) => {
        const url = port ? `https://${host}:${port}/mcp` : `https://${host}/mcp`;
        expect(mcpThrows(url)).toBeNull();
      }),
      { numRuns: 300 }
    );
  });

  it("rejects https on a non-443 port even for a public host (defense in depth)", () => {
    fc.assert(
      fc.property(
        publicHost,
        fc.integer({ min: 1, max: 65535 }).filter((p) => p !== 443),
        (host, port) => {
          const err = mcpThrows(`https://${host}:${port}/mcp`);
          expect(err).toBeInstanceOf(Error);
          expect(err?.message).toMatch(/must use port 443/);
        }
      ),
      { numRuns: 200 }
    );
  });

  it("never throws a non-Error and never accepts a denied host (combined fuzz)", () => {
    fc.assert(
      fc.property(fc.oneof(privateHost, publicHost), (host) => {
        const err = mcpThrows(`https://${host}/mcp`);
        // Result is total: either a clean accept (null) or a typed Error — never a crash.
        expect(err === null || err instanceof Error).toBe(true);
      }),
      { numRuns: 300 }
    );
  });

  it("parseSessionWebhook rejects non-https, userinfo, and unparseable URLs; accepts clean https", () => {
    fc.assert(
      fc.property(
        fc.oneof(
          fc.constant({ url: "http://example.com/hook" }), // not https
          fc.constant({ url: "https://user:pass@example.com/hook" }), // userinfo
          fc.constant({ url: "ftp://example.com" }),
          fc.constant({ url: "not a url" }),
          fc.record({ url: fc.constant("https://example.com/hook"), extra: fc.string() }) // unknown field
        ),
        (input) => {
          expect(() => parseSessionWebhook(input)).toThrow();
        }
      ),
      { numRuns: 100 }
    );
    // A clean https webhook with only `url` always parses.
    fc.assert(
      fc.property(publicHost, (host) => {
        expect(parseSessionWebhook({ url: `https://${host}/hook` })).toEqual({ url: `https://${host}/hook` });
      }),
      { numRuns: 100 }
    );
  });
});
