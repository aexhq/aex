/**
 * Bundle fidelity sidecar SSoT — canonical serialization round-trip, forward-
 * compatible parse, and the SECURITY-critical symlink escape guard (fuzzed).
 */
import { describe, expect, it } from "vitest";
import fc from "fast-check";
import {
  RESERVED_META_ENTRY,
  EXEC_MODE,
  DEFAULT_FILE_MODE,
  bundleManifestIsEmpty,
  parseBundleManifest,
  serializeBundleManifest,
  symlinkTargetEscapes,
  type BundleManifest
} from "../src/bundle-manifest.js";

const dec = new TextDecoder();

describe("sidecar constants", () => {
  it("reserves .aexmeta.json and the two mode buckets", () => {
    expect(RESERVED_META_ENTRY).toBe(".aexmeta.json");
    expect(EXEC_MODE).toBe(0o755);
    expect(DEFAULT_FILE_MODE).toBe(0o644);
  });
});

describe("serializeBundleManifest — canonical + deterministic", () => {
  it("fixed field order, sorted arrays, compact, no trailing newline", () => {
    const m: BundleManifest = {
      v: 1,
      exec: ["z/last", "a/first"],
      symlinks: [
        { path: "z-link", target: "../a" },
        { path: "a-link", target: "b" }
      ]
    };
    const json = dec.decode(serializeBundleManifest(m));
    expect(json).toBe('{"v":1,"exec":["a/first","z/last"],"symlinks":[{"path":"a-link","target":"b"},{"path":"z-link","target":"../a"}]}');
  });

  it("is byte-stable regardless of input array order", () => {
    const a = serializeBundleManifest({ v: 1, exec: ["b", "a"], symlinks: [] });
    const b = serializeBundleManifest({ v: 1, exec: ["a", "b"], symlinks: [] });
    expect([...a]).toEqual([...b]);
  });

  it("round-trips through parseBundleManifest", () => {
    const m: BundleManifest = { v: 1, exec: ["bin/run"], symlinks: [{ path: "cur", target: "rel/target" }] };
    const parsed = parseBundleManifest(serializeBundleManifest(m));
    expect(parsed).toEqual({ v: 1, exec: ["bin/run"], symlinks: [{ path: "cur", target: "rel/target" }] });
  });
});

describe("parseBundleManifest — forward-compatible + defensive", () => {
  it("returns null for absent/empty/garbage bytes", () => {
    expect(parseBundleManifest(null)).toBeNull();
    expect(parseBundleManifest(new Uint8Array(0))).toBeNull();
    expect(parseBundleManifest(new TextEncoder().encode("not json"))).toBeNull();
  });

  it("returns null for an unknown version (metadata dropped, content-only restore)", () => {
    expect(parseBundleManifest(new TextEncoder().encode('{"v":2,"exec":["x"]}'))).toBeNull();
  });

  it("rejects a known-v1 manifest when any exec or symlink entry is malformed", () => {
    const bytes = new TextEncoder().encode(
      '{"v":1,"exec":["ok",123,""],"symlinks":[{"path":"p","target":"t"},{"path":"","target":"x"},{"nope":true}]}'
    );
    expect(parseBundleManifest(bytes)).toBeNull();
    expect(parseBundleManifest(new TextEncoder().encode('{"v":1,"exec":{},"symlinks":[]}'))).toBeNull();
    expect(parseBundleManifest(new TextEncoder().encode('{"v":1,"exec":[],"symlinks":{}}'))).toBeNull();
  });

  it("rejects oversized metadata before serialization", () => {
    const target = "x".repeat(4097);
    expect(() => serializeBundleManifest({
      v: 1,
      exec: [],
      symlinks: [{ path: "link", target }]
    })).toThrow(/4096 characters/);
  });

  it("rejects oversized metadata arrays before iterating them", () => {
    const exec = Array.from({ length: 1_001 }, (_, index) => `bin/run-${index}`);
    const bytes = new TextEncoder().encode(JSON.stringify({ v: 1, exec, symlinks: [] }));
    expect(parseBundleManifest(bytes)).toBeNull();
  });
});

describe("bundleManifestIsEmpty", () => {
  it("true only when both exec and symlinks are empty", () => {
    expect(bundleManifestIsEmpty({ exec: [], symlinks: [] })).toBe(true);
    expect(bundleManifestIsEmpty({})).toBe(true);
    expect(bundleManifestIsEmpty({ exec: ["x"], symlinks: [] })).toBe(false);
    expect(bundleManifestIsEmpty({ exec: [], symlinks: [{ path: "p" }] })).toBe(false);
  });
});

describe("symlinkTargetEscapes — SECURITY boundary", () => {
  it("rejects empty / NUL targets", () => {
    expect(symlinkTargetEscapes("link", "")).toBe(true);
    expect(symlinkTargetEscapes("link", "a\0b")).toBe(true);
  });

  it("rejects absolute + platform-specific targets", () => {
    expect(symlinkTargetEscapes("link", "/etc/passwd")).toBe(true);
    expect(symlinkTargetEscapes("link", "/mnt/session/secrets")).toBe(true);
    expect(symlinkTargetEscapes("link", "C:/Windows")).toBe(true);
    expect(symlinkTargetEscapes("link", "\\\\host\\share")).toBe(true);
    expect(symlinkTargetEscapes("link", "a\\b")).toBe(true);
  });

  it("rejects `..` climbs above the bundle root", () => {
    expect(symlinkTargetEscapes("link", "..")).toBe(true);
    expect(symlinkTargetEscapes("link", "../../etc/shadow")).toBe(true);
    expect(symlinkTargetEscapes("a/b", "../../../x")).toBe(true); // dir depth 1, climbs 3
    expect(symlinkTargetEscapes("a/b/c", "../../../x")).toBe(true); // dir depth 2 (a/b), climbs to root's parent
  });

  it("allows relative in-root targets (incl. dangling + self-dir)", () => {
    expect(symlinkTargetEscapes("cur", "releases/v2")).toBe(false);
    expect(symlinkTargetEscapes("nested/lib", "../shared/lib")).toBe(false); // nested/ + ../shared → shared (in-root)
    expect(symlinkTargetEscapes("a/b/link", "../c")).toBe(false); // a/b + ../c → a/c (in-root)
    expect(symlinkTargetEscapes("a/link", "../a/./b")).toBe(false); // a + ../a/b → a/b
    expect(symlinkTargetEscapes("link", "not/yet/created")).toBe(false); // dangling in-root
  });

  it("a link at root with a single `..` component escapes", () => {
    // link at bundle root, dir = "", target "../shared/lib" → normalize ["..","shared","lib"] → escapes.
    expect(symlinkTargetEscapes("lib", "../shared/lib")).toBe(true);
  });

  it("fuzz: any target lexically resolving above root is rejected; in-root is allowed", { timeout: 0 }, () => {
    const seg = fc.constantFrom("a", "b", "c", "sub", "x");
    const targetArb = fc
      .array(fc.oneof(seg, fc.constant("."), fc.constant("..")), { minLength: 1, maxLength: 6 })
      .map((s) => s.join("/"));
    const linkArb = fc.array(seg, { minLength: 1, maxLength: 4 }).map((s) => s.join("/"));
    fc.assert(
      fc.property(linkArb, targetArb, (link, target) => {
        const rejected = symlinkTargetEscapes(link, target);
        // Independent oracle: does the lexical resolution keep a leading `..`?
        const dir = link.includes("/") ? link.slice(0, link.lastIndexOf("/")) : "";
        const joined = dir ? `${dir}/${target}` : target;
        const stack: string[] = [];
        for (const p of joined.split("/")) {
          if (p === "" || p === ".") continue;
          if (p === "..") {
            if (stack.length && stack[stack.length - 1] !== "..") stack.pop();
            else stack.push("..");
          } else stack.push(p);
        }
        const escapes = stack.length > 0 && stack[0] === "..";
        expect(rejected).toBe(escapes);
      }),
      { numRuns: 500 }
    );
  });
});
