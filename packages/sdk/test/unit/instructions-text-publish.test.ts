/**
 * Publishing instructions uploads NOTHING.
 *
 * `Instructions.fromContent` used to build a canonical single-entry ZIP around
 * `AGENTS.md` and `workspace.instructions.publish` staged that ZIP through
 * `/api/assets/presign` + `/api/assets/finalize` before it could name the
 * resource. Instructions are text now, so the whole staging round trip is gone
 * and `assets:write` is no longer exercised by publishing one.
 *
 * @see references/modular-open-source-2026-07-27/11-archive-registration-redesign.md
 */
import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { hashWorkspaceInstructionText } from "@aexhq/contracts";
import { Aex, Instructions } from "../../src/index.js";

const TEXT = "Follow the repository guide.";

function harness() {
  const calls: Array<{ url: string; method: string; body?: unknown }> = [];
  const fetch: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    calls.push({ url, method: init?.method ?? "GET", ...(body === undefined ? {} : { body }) });
    if (url.includes("/api/workspace/instructions")) {
      return new Response(
        JSON.stringify({
          resource: {
            kind: "instruction",
            resourceId: `wres_${"1".repeat(32)}`,
            version: 1,
            name: "repo-rules",
            textHash: await hashWorkspaceInstructionText(TEXT),
            sizeBytes: TEXT.length,
            createdAt: "2026-07-27T00:00:00.000Z"
          }
        }),
        { status: 201, headers: { "content-type": "application/json" } }
      );
    }
    return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
  };
  return { client: new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch }), calls };
}

describe("instruction drafts carry text", () => {
  it("exposes the trimmed text and its hash, and no bytes", async () => {
    const draft = await Instructions.fromContent(`\n  ${TEXT}  \n`, { name: "repo-rules" });
    const published = draft._takeDraftInstruction();
    expect(published).toEqual({
      name: "repo-rules",
      text: TEXT,
      textHash: await hashWorkspaceInstructionText(TEXT)
    });
    expect("bytes" in published).toBe(false);
  });

  /**
   * §5a of the archive-registration redesign: the 128,000-byte rejection was OUR
   * limit, not a real one, and `a8f34eb9` deleted it. This test asserted the
   * opposite and had been failing ever since -- the SDK half of the same stale
   * assertion that stayed GREEN on the platform side only because its
   * `public-contracts-snapshot` was a stale build of this package.
   *
   * A model's context window is real and is a per-SESSION fact, so the platform
   * PLACES an over-large instruction (staged to a workspace file, prompt carries
   * a pointer) instead of refusing to author it.
   */
  it("imposes NO length bound at authoring time: a model limit is worked around, not refused", async () => {
    const huge = "x".repeat(1_000_000);
    const draft = await Instructions.fromContent(huge, { name: "repo-rules" });
    expect(draft._takeDraftInstruction().text).toHaveLength(1_000_000);
  });
});

describe("workspace.instructions.publish", () => {
  it("POSTs { name, text } and stages no asset", async () => {
    const { client, calls } = harness();
    const record = await client.workspace.instructions.publish(
      await Instructions.fromContent(TEXT, { name: "repo-rules" })
    );

    expect(calls.map((call) => new URL(call.url).pathname)).toEqual(["/api/workspace/instructions"]);
    expect(calls[0]?.body).toEqual({ name: "repo-rules", text: TEXT });
    expect(record.textHash).toBe(await hashWorkspaceInstructionText(TEXT));
    expect("assetId" in record).toBe(false);
    expect("contentType" in record).toBe(false);
  });
});
