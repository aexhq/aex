import { ASSET_ARCHIVE_LIMITS } from "./session-config.js";

/** Render a byte ceiling the way the message quotes it, so the two cannot drift. */
function mib(bytes: number): string {
  return `${bytes / (1024 * 1024)} MiB`;
}

export function assertArchiveCompressedSize(size: number, source: string): void {
  if (!Number.isSafeInteger(size) || size < 0 || size > ASSET_ARCHIVE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `${source} exceeds the ${mib(ASSET_ARCHIVE_LIMITS.maxCompressedBytes)} compressed limit ` +
        `(got ${Number.isFinite(size) ? size : "an invalid byte count"})`
    );
  }
}

export function assertArchiveExpandedSize(size: number, source: string): void {
  if (!Number.isSafeInteger(size) || size < 0 || size > ASSET_ARCHIVE_LIMITS.maxDecompressedBytes) {
    throw new Error(
      `${source} exceeds the ${mib(ASSET_ARCHIVE_LIMITS.maxDecompressedBytes)} expanded limit ` +
        `(got ${Number.isFinite(size) ? size : "an invalid byte count"})`
    );
  }
}

export function assertArchiveEntryCount(count: number, source: string): void {
  if (!Number.isSafeInteger(count) || count < 0 || count > ASSET_ARCHIVE_LIMITS.maxEntries) {
    throw new Error(
      `${source} exceeds the ${ASSET_ARCHIVE_LIMITS.maxEntries}-entry limit ` +
        `(got ${Number.isFinite(count) ? count : "an invalid entry count"})`
    );
  }
}
