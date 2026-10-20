import { Aex, ApiKey, Model, Tool } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({
    harness: "pi",
    model: Model("openai/gpt-4.1-mini", ApiKey(process.env.OPENAI_API_KEY)),
    tools: [Tool({
      name: "lookup_order",
      description: "Look up an order by id",
      input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"], additionalProperties: false },
      run: ({ id }) => ({ id, status: "shipped" }),
    })],
  });
  console.log((await session.send("Look up order A-1001. Has it shipped?")).output);
  console.log("Session:", session.id);
} finally {
  await aex.close();
}
