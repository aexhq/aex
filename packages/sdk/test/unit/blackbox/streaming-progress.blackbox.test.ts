/**
 * BLACKBOX — real per-token streaming (WS9 / H1).
 *
 * The finding: a customer building a live-progress UI must receive the assistant's
 * text as ORDERED per-token deltas, not one collapsed blob at the end. Asserted
 * two ways through the public surface: the assembled `result.text`, and a LIVE
 * `for await` over the turn stream that observes each delta as it arrives.
 */
import { describe, expect, it } from "vitest";
import type { AexEventView } from "@aexhq/contracts";
import { FakePlatform } from "./fake-platform.js";

const CHUNKS = ["The ", "quick ", "brown ", "fox"] as const;

describe("blackbox: streaming per-token progress", () => {
  it("assembles ordered per-token deltas into the final text", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(
      { model: "claude-haiku-4-5", message: "stream it", apiKeys: { anthropic: "sk-ant" } },
      { chunks: CHUNKS, costUsd: 0.01 }
    );

    // Each delta surfaced as its OWN event, in order — not one blob.
    const deltas = result.events.filter((e) => e.isTextMessage()).map((e) => e.data.text);
    expect(deltas).toEqual([...CHUNKS]);
    expect(result.text).toBe("The quick brown fox");
  });

  it("delivers deltas LIVE over the turn stream as they arrive", async () => {
    const platform = new FakePlatform();
    const handle = await platform.aex.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" }
    });

    const stream = platform.turn(handle, "stream it", { chunks: CHUNKS, costUsd: 0.01 });
    const seen: string[] = [];
    for await (const event of stream as AsyncIterable<AexEventView>) {
      if (event.isTextMessage()) seen.push(event.data.text);
    }
    await stream.done();

    expect(seen).toEqual([...CHUNKS]);
  });
});
