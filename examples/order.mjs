import { Aex, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

const lookupOrder = tool({
  name: "lookup_order",
  description: "Look up an order by id.",
  input: z.object({ id: z.string() }),
  run: ({ id }, ctx) => ctx.finish({ id, status: "shipped" }),
});

const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
aex.sessions.create({
  model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.OPENAI_API_KEY },
  agentloop: pi(),
  tools: [lookupOrder()],
}).then(async session => {
  const after = session.state.lastSequence;
  await session.send("Look up order A-1001. Has it shipped?");
  for await (const event of session.events(after)) {
    if (event.type === "turn_failed") throw new Error(JSON.stringify(event.data));
  }
  console.log(JSON.stringify(await session.transcript(), null, 2));
  console.log("Session:", session.id);
}).catch(error => {
  console.error(error);
  process.exitCode = 1;
});
