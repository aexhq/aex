import { Pi, Aex, ApiKey, Model } from "@aexhq/sdk";
import { z } from "zod";
const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({ harness: Pi(), model: Model("openai/gpt-4.1-mini", ApiKey(process.env.MODEL_API_KEY)) });
  try {
    console.log(await session.send("Ada is 37 years old. Extract her details.", {
      output: { type: z.object({ name: z.string(), age: z.number() }), maxRetries: 2 },
    }));
  } finally { await session.delete(); }
} finally { await aex.close(); }
