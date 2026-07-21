import { ASSET_ARCHIVE_LIMITS } from "./session-config.js";

export function assertArchiveCompressedSize(size: number, source: string): void {
  if (!Number.isSafeInteger(size) || size < 0 || size > ASSET_ARCHIVE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `${source} exceeds the 64 MiB compressed limit ` +
        `(got ${Number.isFinite(size) ? size : "an invalid byte count"})`
    );
  }
}

export function assertArchiveExpandedSize(size: number, source: string): void {
  if (!Number.isSafeInteger(size) || size < 0 || size > ASSET_ARCHIVE_LIMITS.maxDecompressedBytes) {
    throw new Error(
      `${source} exceeds the 128 MiB expanded limit ` +
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
