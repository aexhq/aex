import { describe, expect, it } from "vitest";
import { ASSET_ARCHIVE_LIMITS } from "@aexhq/contracts";
import {
  assertCliExpandedSize,
  buildCliFile,
  buildCliInstructions
} from "../src/host/start-submit.js";

describe("CLI runtime asset archive limits", () => {
  it("accepts the exact expanded boundary and rejects one byte above it", () => {
    expect(() => assertCliExpandedSize(ASSET_ARCHIVE_LIMITS.maxDecompressedBytes, "asset")).not.toThrow();
    expect(() => assertCliExpandedSize(ASSET_ARCHIVE_LIMITS.maxDecompressedBytes + 1, "asset"))
      .toThrow(/128 MiB expanded limit/);
  });

  it("rejects an oversized file before zip allocation", async () => {
    const bytes = new Proxy(new Uint8Array(0), {
      get(target, property) {
        if (property === "byteLength") return ASSET_ARCHIVE_LIMITS.maxDecompressedBytes + 1;
        return Reflect.get(target, property, target) as unknown;
      }
    });
    await expect(buildCliFile({ name: "large.bin", bytes })).rejects.toThrow(/128 MiB expanded limit/);
  });

  it("keeps the instruction constructor on the shared size guard", async () => {
    await expect(buildCliInstructions("instructions", "test-instructions")).resolves.toMatchObject({
      name: "test-instructions"
    });
  });
});

describe("CLI instruction resource names", () => {
  it.each([
    "a",
    "a".repeat(64),
    "a".repeat(128),
    "UpperCase",
    "repo.rules",
    "repo_rules",
    "repo-rules"
  ])("preserves accepted name %j", async (name) => {
    await expect(buildCliInstructions("instructions", name)).resolves.toMatchObject({ name });
  });

  it.each([
    "",
    "a".repeat(129),
    "-leading",
    "two words",
    "slash/name",
    "café",
    "bad__name"
  ])("rejects invalid name %j with authoring provenance", async (name) => {
    await expect(buildCliInstructions("instructions", name))
      .rejects.toThrow(/^Instructions\.fromContent: name /);
  });
});
