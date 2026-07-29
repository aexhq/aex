/**
 * Instructions are TEXT, not an archive.
 *
 * The `instruction` kind was the only reason the control plane inflated a
 * customer archive on every submit: the whole operation produced one string,
 * the archive's `AGENTS.md`. Registering the text directly removes the archive
 * from the picture, so the pin becomes a hash of the text and there is no asset
 * to address.
 *
 * @see references/modular-open-source-2026-07-27/11-archive-registration-redesign.md
 */
import { describe, expect, it } from "bun:test";
import * as z from "zod/mini";
import {
  CANONICAL_SHA256_DIGEST_PATTERN,
  assertPinnedWorkspaceResource,
  assertWorkspaceInstructionText,
  hashWorkspaceInstructionText,
  parseSubmission,
  type WorkspaceInstructionRef
} from "../src/index.js";
import { WorkspaceInstructionRecordSchema } from "../src/schemas/response-workspace.js";

const RESOURCE_ID = "wres_0123456789abcdef0123456789abcdef";
const TEXT_HASH = `sha256:${"a".repeat(64)}`;

function baseSubmission(assets: unknown): Record<string, unknown> {
  return {
    model: "deepseek/deepseek-v3",
    prompt: ["hello"],
    assets
  };
}

describe("a pinned instruction carries a textHash, never an asset", () => {
  it("accepts a ref whose identity is resourceId + version + textHash", () => {
    const parsed = parseSubmission(
      baseSubmission({
        instructions: [
          { kind: "instruction", resourceId: RESOURCE_ID, version: 1, name: "repo-rules", textHash: TEXT_HASH }
        ]
      })
    );
    expect(parsed.assets.instructions).toEqual([
      { kind: "instruction", resourceId: RESOURCE_ID, version: 1, name: "repo-rules", textHash: TEXT_HASH }
    ] as unknown as readonly WorkspaceInstructionRef[]);
  });

  it("rejects the retired assetId/contentHash pair on an instruction ref", () => {
    expect(() =>
      parseSubmission(
        baseSubmission({
          instructions: [
            {
              kind: "instruction",
              resourceId: RESOURCE_ID,
              version: 1,
              name: "repo-rules",
              assetId: `asset_${"a".repeat(64)}`,
              contentHash: TEXT_HASH
            }
          ]
        })
      )
    ).toThrow(/assetId is not allowed/);
  });

  it("requires the textHash to be a canonical sha256 digest", () => {
    expect(() =>
      assertPinnedWorkspaceResource(
        { kind: "instruction", resourceId: RESOURCE_ID, version: 1, name: "r", textHash: "nope" },
        "submission.assets.instructions[0]"
      )
    ).toThrow(/textHash must be a sha256 digest/);
  });
});

describe("instruction text hashing", () => {
  it("hashes the TRIMMED utf-8 bytes, so surrounding whitespace never forks the pin", async () => {
    const bare = await hashWorkspaceInstructionText("Follow the repository guide.");
    const padded = await hashWorkspaceInstructionText("\n\n  Follow the repository guide.  \n");
    expect(padded).toBe(bare);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(bare)).toBe(true);
  });
});

describe("instruction text carries NO size bound of ours", () => {
  /**
   * The retired bound was 128,000 UTF-8 bytes, derived as a quarter of the
   * smallest served context window. That was OUR limit, not a real constraint —
   * the same mistake as the archive compressed cap this workstream exists to
   * remove. A model's context window is real; a rejection at AUTHORING time is
   * not, because authoring cannot know which model a future submission will
   * name. Overflow is WORKED AROUND at compose time (staged to a workspace file,
   * pointed at from the system prompt), never refused at registration.
   */
  const RETIRED_BOUND = 128_000;

  it("admits text the retired 128,000-byte bound rejected", () => {
    const multibyte = "é".repeat(RETIRED_BOUND / 2 + 1);
    expect(multibyte.length).toBeLessThan(RETIRED_BOUND);
    expect(new TextEncoder().encode(multibyte).byteLength).toBeGreaterThan(RETIRED_BOUND);
    expect(() => assertWorkspaceInstructionText(multibyte, "instructions.text")).not.toThrow();
  });

  it("exports no instruction size constant for the cap to come back under", async () => {
    const surface = (await import("../src/index.js")) as Record<string, unknown>;
    expect(Object.keys(surface).filter((key) => /INSTRUCTION.*(MAX|LIMIT|CAP)/.test(key))).toEqual([]);
  });

  it("rejects empty and whitespace-only text", () => {
    expect(() => assertWorkspaceInstructionText("   \n ", "instructions.text")).toThrow(
      /instructions\.text must be non-empty/
    );
  });
});

describe("an instruction record is not asset-shaped", () => {
  it("drops contentType, assetId and contentHash and keeps sizeBytes", () => {
    const record = {
      kind: "instruction",
      resourceId: RESOURCE_ID,
      name: "repo-rules",
      version: 1,
      textHash: TEXT_HASH,
      sizeBytes: 28,
      createdAt: "2026-07-27T00:00:00.000Z"
    };
    expect(z.safeParse(WorkspaceInstructionRecordSchema, record).success).toBe(true);
    for (const retired of ["assetId", "contentHash", "contentType"] as const) {
      const withRetired = { ...record, [retired]: "x" };
      expect(
        z.safeParse(WorkspaceInstructionRecordSchema, withRetired).success,
        `${retired} must no longer be accepted on an instruction record`
      ).toBe(false);
    }
  });
});
