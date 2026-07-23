/**
 * Property/fuzz net for the canonical zip framer — the core anti-regression proof
 * that the streaming framer never diverges from `zipSync`, and that streaming the
 * bytes through a running hash equals hashing the whole buffer.
 */
import { describe, expect, it } from "bun:test";
import fc from "fast-check";
import { zipSync } from "fflate";
import { createHash } from "node:crypto";
import { frameCanonicalZipSync, streamBundleZip, type ZipEntrySource } from "../../src/canonical-zip.js";

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
const sha = (u8: Uint8Array): string => createHash("sha256").update(u8).digest("hex");

/** An arbitrary bundle: unique forward-slash paths → byte payloads (varied size/compressibility). */
const bundleArb = fc
  .array(
    fc.record({
      // Path segments: keep them file-ish (no slashes-in-segment, no reserved chars).
      path: fc
        .array(
          fc
            .string({ minLength: 1, maxLength: 8 })
            .filter((s) => !/[/\\\0]/.test(s) && s !== "." && s !== ".." && s.trim().length > 0),
          { minLength: 1, maxLength: 3 }
        )
        .map((segs) => segs.join("/")),
      bytes: fc.oneof(
        fc.uint8Array({ minLength: 0, maxLength: 200 }), // small/edge (incl. empty)
        fc.uint8Array({ minLength: 200, maxLength: 4000 }), // incompressible-ish
        fc
          .tuple(fc.integer({ min: 0, max: 255 }), fc.integer({ min: 1, max: 3000 }))
          .map(([b, n]) => new Uint8Array(n).fill(b)) // very compressible
      )
    }),
    { minLength: 1, maxLength: 12 }
  )
  .map((rows) => {
    // Dedup paths (last wins) and sort into canonical order.
    const map = new Map<string, Uint8Array>();
    for (const r of rows) map.set(r.path, r.bytes);
    return [...map.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  });

function zipSyncOrdered(entries: ReadonlyArray<readonly [string, Uint8Array]>): Uint8Array {
  const zippable: Record<string, [Uint8Array, { mtime: Date }]> = {};
  for (const [name, bytes] of entries) zippable[name] = [bytes, { mtime: ZIP_EPOCH }];
  return zipSync(zippable, { level: 6 });
}

async function collect(sources: readonly ZipEntrySource[]): Promise<Uint8Array> {
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

describe("canonical-zip property", () => {
  // Trailing timeout 0 = no limit (bun, like vitest, disables the timer at 0).
  it(
    "frameCanonicalZipSync(m) === zipSync(m) for arbitrary bundles",
    () => {
      fc.assert(
        fc.property(bundleArb, (entries) => {
          expect(sha(frameCanonicalZipSync(entries))).toBe(sha(zipSyncOrdered(entries)));
        }),
        { numRuns: 300 }
      );
    },
    0
  );

  it(
    "streamed framer === in-memory framer for arbitrary bundles",
    async () => {
      await fc.assert(
        fc.asyncProperty(bundleArb, async (entries) => {
          const sources: ZipEntrySource[] = entries.map(([name, bytes]) => ({
            name,
            size: bytes.length,
            read: () => bytes
          }));
          const streamed = await collect(sources);
          expect(sha(streamed)).toBe(sha(frameCanonicalZipSync(entries)));
        }),
        { numRuns: 150 }
      );
    },
    30_000
  );

  it(
    "streamed running-hash === one-shot hash of the whole zip, over random chunkings",
    async () => {
      await fc.assert(
        fc.asyncProperty(bundleArb, fc.integer({ min: 1, max: 997 }), async (entries, chunkStep) => {
          const sources: ZipEntrySource[] = entries.map(([name, bytes]) => ({
            name,
            size: bytes.length,
            read: () => bytes
          }));
          // One-shot: hash the whole assembled buffer.
          const whole = await collect(sources);
          const oneShot = sha(whole);
          // Streamed: feed a running hash re-chunked at an arbitrary boundary.
          const h = createHash("sha256");
          await streamBundleZip(sources, (c) => {
            for (let off = 0; off < c.length; off += chunkStep) {
              h.update(c.subarray(off, Math.min(off + chunkStep, c.length)));
            }
          });
          expect(h.digest("hex")).toBe(oneShot);
        }),
        { numRuns: 100 }
      );
    },
    30_000
  );
});
