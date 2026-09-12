import { Aex, agentloop, brainEnv, component, hostEnv, tool } from "@aexhq/sdk";
import { pathToFileURL } from "node:url";
import { z } from "zod";

for (const name of ["AEX_API_KEY", "MODEL_API_KEY", "AGENTLOOP_FILE"]) {
  if (!process.env[name]) throw new Error(`${name} required`);
}
const aex = new Aex({ ...(process.env.AEX_URL ? { baseUrl: process.env.AEX_URL } : {}), apiKey: process.env.AEX_API_KEY });
const lookup = tool({ name: "lookup", description: "Look up an application value", input: z.object({ id: z.string() }), run: ({ id }) => ({ id, value: "example" }) });
try {
  const session = await aex.sessions.create({
    model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.MODEL_API_KEY },
    agentloop: agentloop({ implementation: component(pathToFileURL(process.env.AGENTLOOP_FILE)) })({ env: brainEnv({ name: "brain" }) }),
    tools: [lookup({ env: hostEnv({ name: "app" }) })],
  });
  try {
    await session.send("Look up item 42");
    for await (const event of session.events()) console.log(event);
  } finally {
    await session.end();
    await session.delete();
  }
} finally {
  await aex.close();
}
