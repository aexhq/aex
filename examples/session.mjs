import { Aex, ApiKey, Model } from "@aexhq/sdk";
const aex = new Aex({ ...(process.env.AEX_URL ? { baseUrl: process.env.AEX_URL } : {}), apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({ harness: "pi", model: Model("openai/gpt-4.1-mini", ApiKey(process.env.MODEL_API_KEY)) });
  try { console.log((await session.send("Hello")).output); }
  finally { await session.delete(); }
} finally { await aex.close(); }
