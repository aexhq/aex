/**
 * GOLDEN determinism tests for the canonical zip framer — the dedup correctness
 * anchor. If any of these fail (an fflate bump, a framer edit), content-addressed
 * dedup is silently broken; fail LOUDLY in CI.
 *
 *   Canonical A — the per-entry framer MUST reproduce `zipSync(..., {level:6})`
 *   BYTE-FOR-BYTE (in-memory and streamed), and a metadata-free bundle must equal
 *   today's output, so existing assets + B1 skill bundles keep deduping.
 *
 *   Canonical B — the pinned-chunk streaming deflate MUST be internally
 *   deterministic (same input → same bytes, independent of input chunking).
 */
import { describe, expect, it } from "vitest";
import { zipSync, unzipSync } from "fflate";
import { createHash } from "node:crypto";
import {
  ENTRY_RAM_CAP,
  frameCanonicalZipSync,
  streamBundleZip,
  type ZipEntrySource
} from "../../src/canonical-zip.js";

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
const enc = new TextEncoder();
const sha = (u8: Uint8Array): string => "sha256:" + createHash("sha256").update(u8).digest("hex");

function patterned(n: number, seed: number): Uint8Array {
  const a = new Uint8Array(n);
  for (let i = 0; i < n; i++) a[i] = (i * seed + 7) & 0xff;
  return a;
}

/** Frozen fixture: empty entry, single byte, binary, unicode + astral filename, exec-y path. */
const FIXTURE_A: Array<[string, Uint8Array]> = [
  ["SKILL.md", enc.encode("# skill\n")],
  ["bin/run.sh", enc.encode("#!/bin/sh\necho hi\n")],
  ["data/a.bin", patterned(1024, 7)],
  ["empty", new Uint8Array(0)],
  ["one", new Uint8Array([42])],
  ["unicode/café-🚀.txt", enc.encode("üñî")]
];

/** Pinned hashes — a change here means the on-the-wire zip bytes changed (dedup break). */
const GOLDEN_A_SHA = "sha256:d27ed086127d701b4b3ebe64326cb05bde88ee1a549cc205a934a1e77c0dc14f";
const GOLDEN_B_SHA = "sha256:65a5c2531a327e01d6a171d3971f20a94224d638f40b83b09b94ceb92913d832";

function zipSyncOrdered(entries: ReadonlyArray<readonly [string, Uint8Array]>): Uint8Array {
  const zippable: Record<string, [Uint8Array, { mtime: Date }]> = {};
  for (const [name, bytes] of entries) zippable[name] = [bytes, { mtime: ZIP_EPOCH }];
  return zipSync(zippable, { level: 6 });
}

async function collectStream(sources: readonly ZipEntrySource[]): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  await streamBundleZip(sources, (c) => {
    chunks.push(c);
  });
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let o = 0;
  for (const c of chunks) {
    out.set(c, o);
    o += c.length;
  }
  return out;
}

describe("canonical A — byte-identical to zipSync", () => {
  it("sync framer == zipSync, byte-for-byte, and matches the pinned golden hash", () => {
    const framed = frameCanonicalZipSync(FIXTURE_A);
    const ref = zipSyncOrdered(FIXTURE_A);
    expect(sha(framed)).toBe(sha(ref));
    expect(sha(framed)).toBe(GOLDEN_A_SHA);
  });

  it("streamed framer == zipSync, byte-for-byte", async () => {
    const sources: ZipEntrySource[] = FIXTURE_A.map(([name, bytes]) => ({
      name,
      size: bytes.length,
      read: () => bytes
    }));
    const streamed = await collectStream(sources);
    expect(sha(streamed)).toBe(sha(zipSyncOrdered(FIXTURE_A)));
  });

  it("a no-metadata bundle equals today's plain zipSync output (dedup continuity)", () => {
    // No sidecar appended → content-only. Must match a plain sorted zipSync.
    const contentOnly = FIXTURE_A;
    expect(sha(frameCanonicalZipSync(contentOnly))).toBe(sha(zipSyncOrdered(contentOnly)));
  });

  it("round-trips every entry through unzipSync", () => {
    const un = unzipSync(frameCanonicalZipSync(FIXTURE_A));
    for (const [name, bytes] of FIXTURE_A) {
      expect(sha(un[name]!)).toBe(sha(bytes));
    }
  });

  it("edge shapes match zipSync (empty entry, single byte, incompressible)", () => {
    const cases: Array<[string, Uint8Array]>[] = [
      [["only-empty", new Uint8Array(0)]],
      [["single", new Uint8Array([7])]],
      [["rand", patterned(4096, 251)]]
    ];
    for (const entries of cases) {
      expect(sha(frameCanonicalZipSync(entries))).toBe(sha(zipSyncOrdered(entries)));
    }
  });
});

describe("canonical B — internally deterministic", () => {
  const big = patterned(5 * 1024 * 1024 + 123, 13);

  function bSource(step: number): ZipEntrySource[] {
    return [
      {
        name: "dataset.bin",
        size: ENTRY_RAM_CAP + 1, // force the B path
        openStream: async function* () {
          for (let off = 0; off < big.length; off += step) {
            yield big.subarray(off, Math.min(off + step, big.length));
          }
        },
        read: () => {
          throw new Error("giant entry must stream");
        }
      }
    ];
  }

  it("matches the pinned golden hash", async () => {
    const bytes = await collectStream(bSource(700003));
    expect(sha(bytes)).toBe(GOLDEN_B_SHA);
  });

  it("is independent of input chunking (re-chunks to the pinned push size)", async () => {
    const a = await collectStream(bSource(700003));
    const b = await collectStream(bSource(1_000_000));
    const c = await collectStream(bSource(333));
    expect(sha(a)).toBe(sha(b));
    expect(sha(b)).toBe(sha(c));
  });

  it("uses a streaming data descriptor (GP-flag bit 3 set) and round-trips", async () => {
    const bytes = await collectStream(bSource(700003));
    expect(bytes[6]).toBe(8); // GP-flag low byte = 0x08 (bit 3, data descriptor)
    expect(sha(unzipSync(bytes)["dataset.bin"]!)).toBe(sha(big));
  });
});
