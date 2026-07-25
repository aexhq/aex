import { describe, expect, it } from "bun:test";
import { ASSET_ARCHIVE_LIMITS } from "@aexhq/contracts";
import {
  assertArchiveCompressedSize,
  assertArchiveEntryCount,
  assertArchiveExpandedSize
} from "../../src/archive-limits.js";

describe("runtime asset archive limits", () => {
  it("accepts exact compressed and expanded boundaries", () => {
    expect(() => assertArchiveCompressedSize(ASSET_ARCHIVE_LIMITS.maxCompressedBytes, "asset"))
      .not.toThrow();
    expect(() => assertArchiveExpandedSize(ASSET_ARCHIVE_LIMITS.maxDecompressedBytes, "asset"))
      .not.toThrow();
    expect(() => assertArchiveEntryCount(ASSET_ARCHIVE_LIMITS.maxEntries, "asset")).not.toThrow();
  });

  it("rejects one byte beyond either boundary with stable units", () => {
    expect(() => assertArchiveCompressedSize(ASSET_ARCHIVE_LIMITS.maxCompressedBytes + 1, "asset"))
      .toThrow(/16 MiB compressed limit/);
    expect(() => assertArchiveExpandedSize(ASSET_ARCHIVE_LIMITS.maxDecompressedBytes + 1, "asset"))
      .toThrow(/128 MiB expanded limit/);
    expect(() => assertArchiveEntryCount(ASSET_ARCHIVE_LIMITS.maxEntries + 1, "asset"))
      .toThrow(/1000-entry limit/);
  });
});
