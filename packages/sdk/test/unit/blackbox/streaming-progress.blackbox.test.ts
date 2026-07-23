/**
 * BLACKBOX — real per-token streaming (WS9 / H1).
 *
 * The finding: a customer building a live-progress UI must receive the assistant's
 * text as ORDERED per-token deltas, not one collapsed blob at the end. Asserted
 * two ways through the public surface: the assembled `result.text`, and a LIVE
 * `for await` over the turn stream that observes each delta as it arrives.
 */
import { describe, expect, it } from "bun:test";
import type { AexStreamEventView } from "@aexhq/contracts";
import { FakePlatform } from "./fake-platform.js";

const CHUNKS = ["The ", "quick ", "brown ", "fox"] as const;

describe("blackbox: streaming per-token progress", () => {
  it("assembles ordered per-token deltas into the final text", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(
      { model: "claude-haiku-4-5", message: "stream it", outputMode: "stream", apiKeys: { anthropic: "sk-ant" } },
      { chunks: CHUNKS, costUsd: 0.01 }
    );

    // Finished results contain only the durable coalesced message, never the
    // provisional deltas that were already shown live.
    const messages = result.events.filter((e) => e.isTextMessage()).map((e) => e.data.text);
    expect(messages).toEqual([CHUNKS.join("")]);
    expect(result.text).toBe("The quick brown fox");
    expect(result.messages.map((message) => message.text)).toEqual([CHUNKS.join("")]);
  });

  it("delivers deltas LIVE over the turn stream as they arrive", async () => {
    const platform = new FakePlatform();
    const handle = await platform.aex.sessions.create({
      model: "claude-haiku-4-5",
      outputMode: "stream",
      apiKeys: { anthropic: "sk-ant" }
    });

    const stream = platform.turn(handle, "stream it", { chunks: CHUNKS, costUsd: 0.01 });
    const seen: string[] = [];
    const liveSequences: number[] = [];
    const replayableFlags: boolean[] = [];
    const durableSequencePresence: boolean[] = [];
    for await (const event of stream as AsyncIterable<AexStreamEventView>) {
      if (event.replayable === false && event.isTextMessage() && event.data.delta === true) {
        seen.push(event.data.text);
        replayableFlags.push(event.replayable);
        durableSequencePresence.push("sequence" in event);
        liveSequences.push(event.liveSequence);
      }
    }
    await stream.finished();

    expect(seen).toEqual([...CHUNKS]);
    expect(liveSequences).toEqual([0, 1, 2, 3]);
    expect(replayableFlags).toEqual([false, false, false, false]);
    expect(durableSequencePresence).toEqual([false, false, false, false]);
  });
});
