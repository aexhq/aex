import { serveHarness } from "@aexhq/brain/harness";

// A completed turn without public assistant output must not start correction turns.
await serveHarness({ name: "no-output-fixture", stateVersion: "1", async open() {
  return { async run(ctx) {
    const messages = [...ctx.transcript, { role: "user", content: [{ type: "text", text: ctx.input.message }] }];
    const result = await ctx.model({ system: ctx.system, messages, tools: [] });
    await ctx.setTranscript([...messages, result.message]);
  }, close() {} };
} });
