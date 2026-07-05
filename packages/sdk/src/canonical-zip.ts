/**
 * Canonical zip framer — the deterministic, content-addressed zip layer the
 * asset upload path dedups on. Two canonical forms, both byte-stable:
 *
 *   Canonical A (the ~99% case: every entry ≤ {@link ENTRY_RAM_CAP}) — a custom
 *   per-entry framer that reproduces fflate's `zipSync(..., {level:6})` output
 *   BYTE-FOR-BYTE (sorted entries, epoch mtime, per-entry crc32, per-entry
 *   one-shot `deflateSync` level 6, real sizes in a flag-0 local header) while
 *   holding only ONE entry in memory at a time. Because it equals `zipSync`, a
 *   streamed large bundle dedups against a small in-memory bundle of the same
 *   content — no dedup seam. Pinned forever by a golden byte-identity test.
 *
 *   Canonical B (a bundle containing ANY entry > {@link ENTRY_RAM_CAP}, e.g. a
 *   lone multi-GB dataset file) — fflate's streaming `Zip`/`ZipDeflate` driven
 *   with a PINNED {@link CANONICAL_B_PUSH_BYTES} push size. It does NOT equal
 *   `zipSync` (it emits streaming data descriptors, GP-flag bit 3), but it IS
 *   internally deterministic: same input → same bytes every time (pinned chunk
 *   size + pinned fflate version). Its own second canonical / dedup namespace,
 *   pinned by its own golden determinism test. A single giant entry cannot be
 *   both `zipSync`-identical AND framed in O(part) memory, so this is the
 *   accepted tradeoff (such a file has no small twin to dedup against).
 *
 * Browser-safe: this module imports ONLY fflate (no `node:*`). The node-only
 * streaming SHA-256 + multipart sink live in the callers (file.ts / asset-upload).
 */

import { deflateSync, strToU8, Zip, ZipDeflate } from "fflate";

/** Epoch every entry mtime is pinned to (matches the pre-streaming `zipSync` path). */
export const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));

/**
 * Per-entry raw-byte ceiling for Canonical A. A single entry above this cannot be
 * one-shot `deflateSync`-ed within a bounded memory budget, so a bundle carrying
 * any such entry falls to Canonical B. A memory-budget knob, NOT a correctness
 * boundary (the golden tests pin both forms).
 */
export const ENTRY_RAM_CAP = 512 * 1024 * 1024;

/** Pinned push size for Canonical B's streaming deflate. LOAD-BEARING for determinism. */
export const CANONICAL_B_PUSH_BYTES = 1024 * 1024;

// ---------------------------------------------------------------------------
// CRC-32 (IEEE, poly 0xEDB88320) — matches zlib/fflate, verified byte-identical.
// ---------------------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

/** One-shot CRC-32 over `bytes`. */
export function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = (CRC_TABLE[(c ^ bytes[i]!) & 0xff]! ^ (c >>> 8)) >>> 0;
  return (c ^ 0xffffffff) >>> 0;
}

// ---------------------------------------------------------------------------
// Entry source + sink abstractions.
// ---------------------------------------------------------------------------

/** A byte sink; may apply backpressure (pass-2 multipart) or run sync (pass-1 hash). */
export type ByteSink = (chunk: Uint8Array) => void | Promise<void>;

/**
 * One bundle entry to frame, in FINAL canonical order. `name` is the forward-slash
 * bundle-relative path; `size` its raw byte length (from `stat` or the in-memory
 * bytes). `read()` yields the whole entry (Canonical A / small); `openStream()` is
 * required only when `size > ENTRY_RAM_CAP` (Canonical B).
 */
export interface ZipEntrySource {
  readonly name: string;
  readonly size: number;
  read(): Uint8Array | Promise<Uint8Array>;
  openStream?(): AsyncIterable<Uint8Array>;
}

/** True when the bundle must use Canonical B (some entry exceeds the RAM cap). */
export function bundleNeedsCanonicalB(entries: readonly { readonly size: number }[]): boolean {
  return entries.some((e) => e.size > ENTRY_RAM_CAP);
}

/**
 * Reorder entries into the EXACT order `zipSync` emits them. `zipSync` iterates
 * its input object with `for..in`, so integer-index-like keys (e.g. a file
 * literally named `"1"`) enumerate FIRST in ascending numeric order, then the
 * rest in insertion order. Delegating to `Object.keys` reproduces that engine
 * rule verbatim, so the framer stays byte-identical for every input (the callers
 * pass entries already sorted, which becomes the insertion order for non-integer
 * keys). Duplicate names collapse to the last occurrence, matching object build.
 */
function canonicalOrder<T extends { readonly name: string }>(entries: readonly T[]): T[] {
  const marker: Record<string, true> = {};
  const byName = new Map<string, T>();
  for (const e of entries) {
    marker[e.name] = true;
    byName.set(e.name, e);
  }
  return Object.keys(marker).map((name) => byName.get(name)!);
}

// ---------------------------------------------------------------------------
// Little-endian header writers.
// ---------------------------------------------------------------------------

function pushU16(out: number[], v: number): void {
  out.push(v & 0xff, (v >>> 8) & 0xff);
}
function pushU32(out: number[], v: number): void {
  const u = v >>> 0;
  out.push(u & 0xff, (u >>> 8) & 0xff, (u >>> 16) & 0xff, (u >>> 24) & 0xff);
}

/** DOS mod-time / mod-date words for a UTC Date (matches fflate's encoding). */
function dosTimeDate(dt: Date): { time: number; date: number } {
  const time = (dt.getUTCHours() << 11) | (dt.getUTCMinutes() << 5) | (dt.getUTCSeconds() >> 1);
  const date = ((dt.getUTCFullYear() - 1980) << 9) | ((dt.getUTCMonth() + 1) << 5) | dt.getUTCDate();
  return { time: time & 0xffff, date: date & 0xffff };
}

const { time: DOS_TIME, date: DOS_DATE } = dosTimeDate(ZIP_EPOCH);

// ---------------------------------------------------------------------------
// Canonical A — per-entry framer (byte-identical to zipSync).
// ---------------------------------------------------------------------------

interface FramedEntryA {
  readonly local: Uint8Array; // local file header + deflated payload
  readonly central: Uint8Array; // central directory record
  readonly length: number; // bytes consumed in the local section (== local.length)
}

/** Frame one Canonical-A entry: local header (flag 0, real sizes) + deflate-L6 payload. */
function frameEntryA(name: string, raw: Uint8Array, offset: number): FramedEntryA {
  // Encode the name with fflate's OWN `strToU8` (what `zipSync` uses) so the
  // filename bytes are byte-identical to `zipSync` for every input, including
  // edge unicode (lone surrogates etc.) where TextEncoder would diverge.
  const nameBytes = strToU8(name);
  const payload = deflateSync(raw, { level: 6 });
  const crc = crc32(raw);
  // GP flag: low byte = dbf(level 6) << 1 = 0; high byte = 0x08 (bit 11, UTF-8)
  // when the name is non-ASCII — fflate sets it iff the encoded byte length
  // differs from the JS string length. Matching it keeps unicode names identical.
  const gpFlag = nameBytes.length !== name.length ? 0x0800 : 0;

  const local: number[] = [];
  pushU32(local, 0x04034b50);
  pushU16(local, 20); // version needed
  pushU16(local, gpFlag); // GP flag
  pushU16(local, 8); // method: deflate
  pushU16(local, DOS_TIME);
  pushU16(local, DOS_DATE);
  pushU32(local, crc);
  pushU32(local, payload.length);
  pushU32(local, raw.length);
  pushU16(local, nameBytes.length);
  pushU16(local, 0); // extra len
  const header = Uint8Array.from(local);

  const localBytes = new Uint8Array(header.length + nameBytes.length + payload.length);
  localBytes.set(header, 0);
  localBytes.set(nameBytes, header.length);
  localBytes.set(payload, header.length + nameBytes.length);

  const central: number[] = [];
  pushU32(central, 0x02014b50);
  pushU16(central, 20); // version made by
  pushU16(central, 20); // version needed
  pushU16(central, gpFlag); // GP flag
  pushU16(central, 8); // method
  pushU16(central, DOS_TIME);
  pushU16(central, DOS_DATE);
  pushU32(central, crc);
  pushU32(central, payload.length);
  pushU32(central, raw.length);
  pushU16(central, nameBytes.length);
  pushU16(central, 0); // extra len
  pushU16(central, 0); // comment len
  pushU16(central, 0); // disk number
  pushU16(central, 0); // internal attrs
  pushU32(central, 0); // external attrs
  pushU32(central, offset); // local header offset
  const centralHeader = Uint8Array.from(central);
  const centralBytes = new Uint8Array(centralHeader.length + nameBytes.length);
  centralBytes.set(centralHeader, 0);
  centralBytes.set(nameBytes, centralHeader.length);

  return { local: localBytes, central: centralBytes, length: localBytes.length };
}

function eocd(recordCount: number, cdSize: number, cdOffset: number): Uint8Array {
  const out: number[] = [];
  pushU32(out, 0x06054b50);
  pushU16(out, 0); // this disk
  pushU16(out, 0); // cd start disk
  pushU16(out, recordCount);
  pushU16(out, recordCount);
  pushU32(out, cdSize);
  pushU32(out, cdOffset);
  pushU16(out, 0); // comment len
  return Uint8Array.from(out);
}

/**
 * Stream a Canonical-A zip of `entries` (in FINAL canonical order) into `sink`,
 * holding only one entry's raw + compressed bytes in memory at a time. Output is
 * byte-identical to `zipSync(<same ordered entries>, {level:6})`.
 */
async function streamBundleZipA(entries: readonly ZipEntrySource[], sink: ByteSink): Promise<void> {
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const raw = await entry.read();
    const framed = frameEntryA(entry.name, raw, offset);
    await sink(framed.local);
    centrals.push(framed.central);
    offset += framed.length;
  }
  let cdSize = 0;
  for (const c of centrals) cdSize += c.length;
  for (const c of centrals) await sink(c);
  await sink(eocd(centrals.length, cdSize, offset));
}

// ---------------------------------------------------------------------------
// Canonical B — fflate streaming Zip with pinned push size (giant single entry).
// ---------------------------------------------------------------------------

/**
 * Stream a Canonical-B zip into `sink` using fflate's streaming `Zip`, pushing
 * each entry in fixed {@link CANONICAL_B_PUSH_BYTES} chunks. Deterministic for a
 * given input + fflate version. Entries are added in the given (canonical) order.
 */
async function streamBundleZipB(entries: readonly ZipEntrySource[], sink: ByteSink): Promise<void> {
  const pending: Uint8Array[] = [];
  const zip = new Zip();
  zip.ondata = (err, chunk) => {
    if (err) throw err;
    if (chunk && chunk.length) {
      // Copy: fflate may reuse the chunk buffer after ondata returns.
      const copy = new Uint8Array(chunk.length);
      copy.set(chunk);
      pending.push(copy);
    }
  };
  const drain = async (): Promise<void> => {
    while (pending.length > 0) {
      const chunk = pending.shift()!;
      await sink(chunk);
    }
  };

  for (const entry of entries) {
    const file = new ZipDeflate(entry.name, { level: 6 });
    file.mtime = ZIP_EPOCH; // pin mtime (not a constructor option in the fflate types)
    zip.add(file);
    if (entry.size === 0 || entry.size <= ENTRY_RAM_CAP) {
      // Small entry inside a Canonical-B bundle: one push (still deterministic).
      const raw = await entry.read();
      file.push(raw, true);
      await drain();
      continue;
    }
    // Giant entry: pinned-chunk streaming from its byte stream. Buffer incoming
    // chunks in a queue and assemble EXACTLY-`CANONICAL_B_PUSH_BYTES` pushes
    // (last push carries the remainder) — O(total) copying regardless of the
    // source's chunk sizes, and byte-identical push boundaries → deterministic.
    const openStream = entry.openStream;
    if (!openStream) {
      throw new Error(`canonical-zip: entry ${JSON.stringify(entry.name)} exceeds ENTRY_RAM_CAP but has no openStream()`);
    }
    const queue: Uint8Array[] = [];
    let buffered = 0;
    const emitPush = (size: number, last: boolean): void => {
      const buf = new Uint8Array(size);
      let off = 0;
      while (off < size) {
        const head = queue[0]!;
        const take = Math.min(head.length, size - off);
        buf.set(head.subarray(0, take), off);
        off += take;
        if (take === head.length) queue.shift();
        else queue[0] = head.subarray(take);
      }
      buffered -= size;
      file.push(buf, last);
    };
    for await (const raw of openStream()) {
      if (raw.length === 0) continue;
      queue.push(raw);
      buffered += raw.length;
      while (buffered >= CANONICAL_B_PUSH_BYTES) {
        emitPush(CANONICAL_B_PUSH_BYTES, false);
        await drain();
      }
    }
    // Final push carries the (< push size) remainder, and flags stream end.
    emitPush(buffered, true);
    await drain();
  }
  zip.end();
  await drain();
}

/**
 * Stream the canonical zip of `entries` (FINAL canonical order) into `sink`.
 * Picks Canonical A (byte-identical to `zipSync`) unless an entry exceeds
 * {@link ENTRY_RAM_CAP}, in which case the whole bundle uses Canonical B.
 */
export async function streamBundleZip(entries: readonly ZipEntrySource[], sink: ByteSink): Promise<void> {
  const ordered = canonicalOrder(entries);
  return bundleNeedsCanonicalB(ordered) ? streamBundleZipB(ordered, sink) : streamBundleZipA(ordered, sink);
}

/**
 * Synchronous Canonical-A convenience — frame an ordered `[name, bytes]` list to
 * a single buffer. Used by the golden byte-identity test (`=== zipSync`) and by
 * callers that already hold all bytes in memory. NOT for giant entries.
 */
export function frameCanonicalZipSync(entries: ReadonlyArray<readonly [string, Uint8Array]>): Uint8Array {
  const ordered = canonicalOrder(entries.map(([name, bytes]) => ({ name, bytes })));
  const chunks: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const { name, bytes: raw } of ordered) {
    const framed = frameEntryA(name, raw, offset);
    chunks.push(framed.local);
    centrals.push(framed.central);
    offset += framed.length;
  }
  let cdSize = 0;
  for (const c of centrals) {
    cdSize += c.length;
    chunks.push(c);
  }
  chunks.push(eocd(centrals.length, cdSize, offset));
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
