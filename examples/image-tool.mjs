import { readFile } from "node:fs/promises";
import { Aex, ApiKey, Model } from "@aexhq/sdk";
const aex = new Aex({ apiKey: process.env.AEX_API_KEY, maxCostMicroUsd: 1000000 });
try {
  const session = await aex.sessions.create({ harness: "pi", model: Model("openai/gpt-4.1-mini", ApiKey(process.env.MODEL_API_KEY)) });
  const image = await aex.attachments.upload(session.id, await readFile(process.env.IMAGE_FILE), {
    contentType: "image/png", idempotencyKey: crypto.randomUUID(), downloadBudgetBytes: 10485760,
  });
  console.log((await session.send({ text: "Describe this image", media: [image.media] })).output);
  console.log("History retained in session", session.id);
} finally { await aex.close(); }
