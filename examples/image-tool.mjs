import { readFile } from "node:fs/promises";
import { Aex, brainEnv, hostEnv, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

for (const name of ["AEX_API_KEY", "MODEL_API_KEY", "IMAGE_FILE"]) {
  if (!process.env[name]) throw new Error(`${name} required`);
}
const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
const viewImage = tool({
  name: "view_image",
  description: "View the application's PNG image.",
  input: z.object({}),
  run: async (_, context) => {
    const attachment = await aex.attachments.upload(context.sessionId,
      await readFile(process.env.IMAGE_FILE, { signal: context.signal }), {
        contentType: "image/png", idempotencyKey: `image-${context.sequence}`, signal: context.signal,
      });
    return { type: "aex_tool_output", version: 1, content: "Image ready", media: [attachment.media] };
  },
});
try {
  const session = await aex.sessions.create({
    agentloop: pi({ env: brainEnv({ name: "brain" }) }),
    model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.MODEL_API_KEY },
    tools: [viewImage({ env: hostEnv({ name: "app" }) })],
  });
  await session.send("Use view_image and describe the image.");
  for await (const event of session.events()) {
    if (event.type === "output_emitted") console.log(event.data.message);
  }
  await session.end();
  console.log("History retained in session", session.id);
} finally {
  await aex.close();
}
